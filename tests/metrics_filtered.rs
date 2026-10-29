use opentelemetry_sdk::metrics::SdkMeterProvider;
use tracing::level_filters::LevelFilter;
use tracing::Level;
use tracing_opentelemetry::MetricsLayer;
use tracing_subscriber::prelude::*;

#[test]
fn metrics_layer_does_not_enable_spans_other_layers_filter_out() {
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::sink)
                .with_filter(LevelFilter::INFO),
        )
        .with(MetricsLayer::new(SdkMeterProvider::builder().build()));

    tracing::subscriber::with_default(subscriber, || {
        // Neither layer wants this span: the `fmt` layer is filtered to `INFO`,
        // and the `MetricsLayer` only wants metric events.
        assert!(!tracing::span_enabled!(Level::DEBUG, "debug_span"));
        // The `MetricsLayer` still wants metric events below the `fmt` layer's
        // level.
        assert!(tracing::event_enabled!(
            Level::DEBUG,
            monotonic_counter.hello_world = 1_u64
        ));
    });
}
