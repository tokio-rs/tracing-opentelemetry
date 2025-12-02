use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{SdkTracerProvider, SpanData, SpanExporter, Tracer},
};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::Subscriber;
use tracing::{dispatcher::WeakDispatch, level_filters::LevelFilter, Dispatch};
use tracing_opentelemetry::{get_otel_context, layer};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

#[derive(Clone, Default, Debug)]
struct TestExporter(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for TestExporter {
    async fn export(&self, mut batch: Vec<SpanData>) -> OTelSdkResult {
        let spans = self.0.clone();
        if let Ok(mut inner) = spans.lock() {
            inner.append(&mut batch);
        }
        Ok(())
    }
}

/// A custom tracing layer that uses get_otel_context to access OpenTelemetry contexts
/// from span extensions. This simulates a separate layer that needs to interact with
/// OpenTelemetry data managed by the OpenTelemetryLayer.
#[derive(Clone, Default)]
struct CustomLayer {
    /// Store extracted contexts for verification
    extracted_contexts: Arc<Mutex<Vec<opentelemetry::Context>>>,
    dispatch: Arc<OnceLock<WeakDispatch>>,
}

impl CustomLayer {
    fn get_extracted_contexts(&self) -> Vec<opentelemetry::Context> {
        self.extracted_contexts.lock().unwrap().clone()
    }
}

impl<S> Layer<S> for CustomLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        let _ = self.dispatch.set(subscriber.downgrade());
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        if let Some(weak_dispatch) = self.dispatch.get() {
            // Get the span reference from the registry when the span is entered
            if let Some(span_ref) = ctx.span(id) {
                // Use OpenTelemetryContext to extract the OpenTelemetry context
                let mut extensions = span_ref.extensions_mut();
                if let Some(dispatch) = weak_dispatch.upgrade() {
                    if let Some(otel_context) = get_otel_context(&mut extensions, &dispatch) {
                        // Store the extracted context for verification
                        if let Ok(mut contexts) = self.extracted_contexts.lock() {
                            contexts.push(otel_context);
                        }
                    }
                }
            }
        }
    }
}

fn test_tracer_with_custom_layer() -> (
    Tracer,
    SdkTracerProvider,
    TestExporter,
    CustomLayer,
    impl Subscriber,
) {
    let exporter = TestExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("test");

    let custom_layer = CustomLayer::default();

    let subscriber = tracing_subscriber::registry()
        .with(
            layer()
                .with_tracer(tracer.clone())
                .with_filter(LevelFilter::DEBUG),
        )
        .with(custom_layer.clone())
        .with(tracing_subscriber::fmt::layer().with_filter(LevelFilter::TRACE));

    (tracer, provider, exporter, custom_layer, subscriber)
}

#[test]
fn test_span_ref_ext_from_separate_layer() {
    let (_tracer, provider, exporter, custom_layer, subscriber) = test_tracer_with_custom_layer();

    tracing::subscriber::with_default(subscriber, || {
        // Create a span that will be processed by both the OpenTelemetry layer
        // and our custom layer
        let _span = tracing::debug_span!("test_span", test_field = "test_value").entered();

        // Create a child span to test hierarchical context extraction
        let _child_span = tracing::debug_span!("child_span", child_field = "child_value").entered();
    });

    drop(provider); // flush all spans

    // Verify that spans were exported by the OpenTelemetry layer
    let spans = exporter.0.lock().unwrap();
    assert_eq!(spans.len(), 2, "Expected 2 spans to be exported");

    // Verify that our custom layer extracted OpenTelemetry contexts
    let extracted_contexts = custom_layer.get_extracted_contexts();
    assert_eq!(
        extracted_contexts.len(),
        2,
        "Expected 2 contexts to be extracted by custom layer"
    );

    // Verify that the extracted contexts contain valid span contexts
    for (i, context) in extracted_contexts.iter().enumerate() {
        let span = context.span();
        let span_context = span.span_context();
        assert!(
            span_context.is_valid(),
            "Context {} should have a valid span context",
            i
        );
        assert_ne!(
            span_context.trace_id(),
            opentelemetry::trace::TraceId::INVALID,
            "Context {} should have a valid trace ID",
            i
        );
        assert_ne!(
            span_context.span_id(),
            opentelemetry::trace::SpanId::INVALID,
            "Context {} should have a valid span ID",
            i
        );
    }

    // Verify that the contexts correspond to the exported spans
    let parent_span = spans.iter().find(|s| s.name == "test_span").unwrap();
    let child_span = spans.iter().find(|s| s.name == "child_span").unwrap();

    // The first extracted context should correspond to the parent span
    let parent_context = &extracted_contexts[0];
    assert_eq!(
        parent_context.span().span_context().span_id(),
        parent_span.span_context.span_id(),
        "Parent context should match parent span"
    );

    // The second extracted context should correspond to the child span
    let child_context = &extracted_contexts[1];
    assert_eq!(
        child_context.span().span_context().span_id(),
        child_span.span_context.span_id(),
        "Child context should match child span"
    );

    // Verify that both spans share the same trace ID (hierarchical relationship)
    assert_eq!(
        parent_span.span_context.trace_id(),
        child_span.span_context.trace_id(),
        "Parent and child spans should share the same trace ID"
    );

    // Verify that the child span has the parent span as its parent
    assert_eq!(
        child_span.parent_span_id,
        parent_span.span_context.span_id(),
        "Child span should have parent span as its parent"
    );
}
