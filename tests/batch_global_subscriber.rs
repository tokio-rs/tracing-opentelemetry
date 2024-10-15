use futures_util::future::BoxFuture;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    export::trace::{ExportResult, SpanData, SpanExporter},
    runtime,
    trace::TracerProvider,
};
use tokio::runtime::Runtime;
use tracing::{info_span, subscriber, Level, Subscriber};
use tracing_opentelemetry::layer;
use tracing_subscriber::filter;
use tracing_subscriber::prelude::*;

use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
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

fn test_tracer(runtime: &Runtime) -> (TracerProvider, TestExporter, impl Subscriber) {
    let _guard = runtime.enter();

    let exporter = TestExporter::default();
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter.clone(), runtime::Tokio)
        .build();
    let tracer = provider.tracer("test");

    let subscriber = tracing_subscriber::registry().with(
        layer()
            .with_tracer(tracer)
            .with_filter(filter::Targets::new().with_target("test_telemetry", Level::INFO)),
    );

    (provider, exporter, subscriber)
}

#[test]
fn test_global_default() {
    let rt = Runtime::new().unwrap();
    let (provider, exporter, subscriber) = test_tracer(&rt);

    subscriber::set_global_default(subscriber).unwrap();

    for _ in 0..1000 {
        let _span = info_span!(target: "test_telemetry", "test_span").entered();
    }

    // Should flush all batched telemetry spans
    provider.shutdown().unwrap();

    let spans = exporter.0.lock().unwrap();
    assert_eq!(spans.len(), 1000);
}
