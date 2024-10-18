use futures_util::future::BoxFuture;
use opentelemetry::trace::{Status, TracerProvider as _};
use opentelemetry_sdk::{
    export::trace::{ExportResult, SpanData, SpanExporter},
    trace::{Tracer, TracerProvider},
};
use std::sync::{Arc, Mutex};
use tracing::level_filters::LevelFilter;
use tracing::Subscriber;
use tracing_opentelemetry::{layer, OpenTelemetrySpanExt};
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
                .with_filter(LevelFilter::DEBUG),
        )
        .with(tracing_subscriber::fmt::layer().with_filter(LevelFilter::TRACE));

    (tracer, provider, exporter, subscriber)
}

#[test]
fn set_status_ok() {
    let root_span = set_status_helper(Status::Ok);
    assert_eq!(Status::Ok, root_span.status);
}

#[test]
fn set_status_error() {
    let expected_error = Status::Error {
        description: std::borrow::Cow::Borrowed("Elon put in too much fuel in his rocket!"),
    };
    let root_span = set_status_helper(expected_error.clone());
    assert_eq!(expected_error, root_span.status);
}

fn set_status_helper(status: Status) -> SpanData {
    let (_tracer, provider, exporter, subscriber) = test_tracer();

    tracing::subscriber::with_default(subscriber, || {
        let root = tracing::debug_span!("root").entered();

        root.set_status(status);
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();

    assert_eq!(spans.len(), 1);

    spans.iter().find(|s| s.name == "root").unwrap().clone()
}
