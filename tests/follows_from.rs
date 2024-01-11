use futures_util::future::BoxFuture;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    export::trace::{ExportResult, SpanData, SpanExporter},
    trace::{Tracer, TracerProvider},
};
use std::sync::{Arc, Mutex};
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

    // Note that if we added a `with_filter` here, the original bug (issue #14) will
    // not reproduce. This is because the `Filtered` layer will not
    // call the `tracing-opentelemetry` `Layer`'s `on_follows_from`, as the
    // closed followed span no longer exists in a way that can checked against
    // the that `Filtered`'s filter.
    let subscriber = tracing_subscriber::registry().with(layer().with_tracer(tracer.clone()));

    (tracer, provider, exporter, subscriber)
}

#[test]
fn trace_follows_from_closed() {
    let (_tracer, provider, exporter, subscriber) = test_tracer();

    tracing::subscriber::with_default(subscriber, || {
        let f = tracing::debug_span!("f");
        let f_id = f.id().unwrap();
        drop(f);

        let s = tracing::debug_span!("span");
        // This should not panic
        s.follows_from(f_id);
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();
    // Only the child spans are reported.
    assert_eq!(spans.len(), 2);
}
