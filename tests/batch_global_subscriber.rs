use opentelemetry::{global as otel_global, trace::TracerProvider as _};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{SdkTracerProvider, SpanData, SpanExporter},
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
    async fn export(&self, mut batch: Vec<SpanData>) -> OTelSdkResult {
        let spans = self.0.clone();
        if let Ok(mut inner) = spans.lock() {
            inner.append(&mut batch);
        }
        Ok(())
    }
}

fn test_tracer(runtime: &Runtime) -> (SdkTracerProvider, TestExporter, impl Subscriber) {
    let _guard = runtime.enter();

    let exporter = TestExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter.clone())
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
fn shutdown_in_scope() {
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

#[test]
#[ignore = "https://github.com/open-telemetry/opentelemetry-rust/issues/1961"]
fn shutdown_global() {
    let rt = Runtime::new().unwrap();
    let (provider, exporter, subscriber) = test_tracer(&rt);

    otel_global::set_tracer_provider(provider.clone());
    subscriber::set_global_default(subscriber).unwrap();

    for _ in 0..1000 {
        let _span = info_span!(target: "test_telemetry", "test_span").entered();
    }

    // Should flush all batched telemetry spans
    provider.shutdown().unwrap();

    let spans = exporter.0.lock().unwrap();
    assert_eq!(spans.len(), 1000);
}
