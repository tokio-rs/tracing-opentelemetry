use futures_util::future::BoxFuture;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    export::trace::{ExportResult, SpanData, SpanExporter},
    trace::{Config, SpanLimits, Tracer, TracerProvider},
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

fn test_tracer() -> (
    Tracer,
    TracerProvider,
    TestExporter,
    impl Subscriber + Clone,
) {
    let exporter = TestExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .with_config(Config {
            span_limits: SpanLimits {
                max_events_per_span: u32::MAX,
                ..SpanLimits::default()
            },
            ..Config::default()
        })
        .build();
    let tracer = provider.tracer("test");

    let subscriber = tracing_subscriber::registry()
        .with(
            layer()
                .with_tracer(tracer.clone())
                .with_filter(LevelFilter::TRACE),
        )
        .with(tracing_subscriber::fmt::layer().with_filter(LevelFilter::DEBUG));

    (tracer, provider, exporter, Arc::new(subscriber))
}

#[test]
fn multi_threading() {
    let (_tracer, provider, exporter, subscriber) = test_tracer();

    tracing::subscriber::with_default(subscriber.clone(), || {
        let root = tracing::debug_span!("root");
        std::thread::scope(|scope| {
            for _ in 0..10 {
                scope.spawn(|| {
                    let _guard = tracing::subscriber::set_default(subscriber.clone());
                    let _guard = root.enter();
                    for _ in 0..1000 {
                        tracing::trace!("event");
                    }
                });
            }
        });
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();

    assert_eq!(spans.len(), 1);

    assert_eq!(spans.iter().next().unwrap().events.len(), 10_000);
}
