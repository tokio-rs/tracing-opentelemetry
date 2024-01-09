use futures_util::future::BoxFuture;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    export::trace::{ExportResult, SpanData, SpanExporter},
    trace::{Tracer, TracerProvider},
};
use std::sync::{Arc, Mutex};
use tracing::level_filters::LevelFilter;
use tracing::Subscriber;
use tracing_opentelemetry::layer;
use tracing_subscriber::prelude::*;

#[derive(Clone, Default, Debug)]
struct TestExporter(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for TestExporter {
    fn export(&mut self, mut batch: Vec<SpanData>) -> BoxFuture<'static, ExportResult> {
        let spans = self.0.clone();
        Box::pin(async move {
            if let Ok(mut inner) = spans.lock() {
                inner.append(&mut batch);
            }
            Ok(())
        })
    }
}

fn test_tracer() -> (Tracer, TracerProvider, TestExporter, impl Subscriber) {
    let exporter = TestExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("test");

    let subscriber = tracing_subscriber::registry()
        .with(
            layer()
                .with_tracer(tracer.clone())
                // DEBUG-level, so `trace_spans` are skipped.
                .with_filter(LevelFilter::DEBUG),
        )
        // This is REQUIRED so that the tracing fast path doesn't filter
        // out trace spans at their callsite.
        .with(tracing_subscriber::fmt::layer().with_filter(LevelFilter::TRACE));

    (tracer, provider, exporter, subscriber)
}

#[test]
fn trace_filtered() {
    let (_tracer, provider, exporter, subscriber) = test_tracer();

    tracing::subscriber::with_default(subscriber, || {
        // Neither of these should panic

        let root = tracing::trace_span!("root");
        tracing::debug_span!(parent: &root, "child");

        let root = tracing::trace_span!("root");
        root.in_scope(|| tracing::debug_span!("child"));
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();
    // Only the child spans are reported.
    assert_eq!(spans.len(), 2);
}
