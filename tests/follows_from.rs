use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{SdkTracerProvider, SpanData, SpanExporter, Tracer},
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::Subscriber;
use tracing_opentelemetry::layer;
use tracing_subscriber::prelude::*;
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

fn test_tracer() -> (Tracer, SdkTracerProvider, TestExporter, impl Subscriber) {
    let exporter = TestExporter::default();
    let provider = SdkTracerProvider::builder()
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

#[test]
fn trace_follows_from_partially_closed() {
    /// This layer will take 20ms to process
    #[derive(Clone)]
    struct WaitOnCloseLayer;
    impl<S> Layer<S> for WaitOnCloseLayer
    where
        S: Subscriber,
    {
        fn on_close(&self, _id: tracing::Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            // Some longer running computation
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // Create a test exporter
    let (tracer, provider, _exporter, _subscriber) = test_tracer();
    // And a subscriber with two layers, first our layer, then a slow layer
    let subscriber = Arc::new(
        tracing_subscriber::registry()
            .with(layer().with_tracer(tracer.clone()))
            .with(WaitOnCloseLayer),
    );

    // Now triggering a follows_from in the time where the slow layer is processing `close` should still work.
    tracing::subscriber::with_default(subscriber.clone(), || {
        let cause_span = tracing::debug_span!("f");
        let cause_id = cause_span.id().unwrap();

        // mutli-threading similar to `parallel.rs`
        std::thread::scope(|scope| {
            // One thread will drop the `follows` span, triggering the removal of `OtelData` extension data, and an artificial 20ms wait
            scope.spawn(|| {
                drop(cause_span);
            });
            std::thread::sleep(Duration::from_millis(5));

            // Another thread will make a following span based on the stored id, which triggers `on_follows_from`.
            // However, the wait has not finished and thus the registry context has not been cleared.
            let s = tracing::debug_span!("span");
            // This should not panic (but did prior to fix of #183)
            s.follows_from(cause_id);
        });
    });

    drop(provider); // flush all spans
}
