use futures_util::future::BoxFuture;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    export::trace::{ExportResult, SpanData, SpanExporter},
    trace::{Tracer, TracerProvider},
};
use std::sync::{Arc, Mutex};
use tracing::{instrument, Subscriber};
use tracing_opentelemetry::layer;
use tracing_subscriber::prelude::*;

#[test]
fn map_error_event_to_status_description() {
    let (_tracer, provider, exporter, subscriber) = test_tracer(Some(false), None);

    #[instrument(err)]
    fn test_fn() -> Result<(), &'static str> {
        Err("test error")
    }

    tracing::subscriber::with_default(subscriber, || {
        let _ = test_fn();
    });

    drop(provider); // flush all spans

    // Ensure the error event is mapped to the status description
    let spans = exporter.0.lock().unwrap();
    let span = spans.iter().find(|s| s.name == "test_fn").unwrap();
    assert!(span.status == opentelemetry::trace::Status::error("test error"));
}

#[test]
fn error_mapping_disabled() {
    let (_tracer, provider, exporter, subscriber) = test_tracer(Some(false), Some(false));

    #[instrument(err)]
    fn test_fn() -> Result<(), &'static str> {
        Err("test error")
    }

    tracing::subscriber::with_default(subscriber, || {
        let _ = test_fn();
    });

    drop(provider); // flush all spans

    // Ensure the error event is not mapped to the status description
    let spans = exporter.0.lock().unwrap();
    let span = spans.iter().find(|s| s.name == "test_fn").unwrap();
    assert!(span.status == opentelemetry::trace::Status::error(""));

    let exception_event = span.events.iter().any(|e| e.name == "exception");
    assert!(!exception_event);
}

#[test]
fn transform_error_event_to_exception_event() {
    let (_tracer, provider, exporter, subscriber) = test_tracer(None, Some(false));

    #[instrument(err)]
    fn test_fn() -> Result<(), &'static str> {
        Err("test error")
    }

    tracing::subscriber::with_default(subscriber, || {
        let _ = test_fn();
    });

    drop(provider); // flush all spans

    // Ensure that there is an exception event created and it contains our error.
    let spans = exporter.0.lock().unwrap();
    let span = spans.iter().find(|s| s.name == "test_fn").unwrap();
    let exception_event = span.events.iter().find(|e| e.name == "exception").unwrap();
    let exception_attribute = exception_event
        .attributes
        .iter()
        .find(|a| a.key.as_str() == "exception.message")
        .unwrap();
    assert!(exception_attribute.value.as_str() == "test error");
}

fn test_tracer(
    // Uses options to capture changes of the default behavior
    error_event_exceptions: Option<bool>,
    error_event_status: Option<bool>,
) -> (Tracer, TracerProvider, TestExporter, impl Subscriber) {
    let exporter = TestExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("test");

    let mut layer = layer().with_tracer(tracer.clone());
    if let Some(error_event_exceptions) = error_event_exceptions {
        layer = layer.with_error_events_to_exceptions(error_event_exceptions)
    }
    if let Some(error_event_status) = error_event_status {
        layer = layer.with_error_events_to_status(error_event_status)
    }
    let subscriber = tracing_subscriber::registry().with(layer);

    (tracer, provider, exporter, subscriber)
}

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
