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
                .with_filter(LevelFilter::DEBUG),
        )
        .with(tracing_subscriber::fmt::layer().with_filter(LevelFilter::TRACE));

    (tracer, provider, exporter, subscriber)
}

#[test]
fn explicit_parents_of_events() {
    let (_tracer, provider, exporter, subscriber) = test_tracer();

    tracing::subscriber::with_default(subscriber, || {
        let root = tracing::debug_span!("root").entered();

        tracing::debug!("1");
        tracing::debug!(parent: &root, "2");
        tracing::debug!(parent: None, "3");

        let child = tracing::debug_span!(parent: &root, "child");
        child.in_scope(|| {
            tracing::debug!("4");
            tracing::debug!(parent: &root, "5");
            tracing::debug!(parent: &child, "6");
            tracing::debug!(parent: None, "7");
        });

        tracing::debug!("8");
        tracing::debug!(parent: &root, "9");
        tracing::debug!(parent: &child, "10");
        tracing::debug!(parent: None, "11");

        let root = root.exit();

        tracing::debug!("12");
        tracing::debug!(parent: &root, "13");
        tracing::debug!(parent: &child, "14");
        tracing::debug!(parent: None, "15");
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();

    assert_eq!(spans.len(), 2);

    {
        // Check the root span
        let expected_root_events = ["1", "2", "5", "8", "9", "13"];

        let root_span = spans.iter().find(|s| s.name == "root").unwrap();
        let actual_events: Vec<_> = root_span
            .events
            .iter()
            .map(|event| event.name.to_string())
            .collect();

        assert_eq!(&expected_root_events, &actual_events[..]);
    }

    {
        // Check the child span
        let expected_child_events = ["4", "6", "10", "14"];

        let child_span = spans.iter().find(|s| s.name == "child").unwrap();
        let actual_events: Vec<_> = child_span
            .events
            .iter()
            .map(|event| event.name.to_string())
            .collect();

        assert_eq!(&expected_child_events, &actual_events[..]);
    }
}
