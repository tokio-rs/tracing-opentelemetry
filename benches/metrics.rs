use criterion::{criterion_group, criterion_main, Criterion};
use opentelemetry::metrics::noop::NoopMeterProvider;
use tracing_opentelemetry::MetricsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn metrics_events(c: &mut Criterion) {
    let mut group = c.benchmark_group("otel_metrics_events");
    {
        let _subscriber = tracing_subscriber::registry().set_default();
        group.bench_function("no_metrics_layer", |b| {
            b.iter(|| {
                tracing::info!(key_1 = "va", "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("no_metrics_events", |b| {
            b.iter(|| {
                tracing::info!(key_1 = "va", "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("1_metrics_event", |b| {
            b.iter(|| {
                tracing::info!(monotonic_counter.foo = 1, "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("2_metrics_events", |b| {
            b.iter(|| {
                tracing::info!(monotonic_counter.foo = 1, monotonic_counter.bar = 1, "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("1_metrics_event_with_attributes", |b| {
            b.iter(|| {
                tracing::info!(monotonic_counter.foo = 1, attr_1 = 10, "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("1_metrics_event_with_attributes_8", |b| {
            b.iter(|| {
                tracing::info!(
                    monotonic_counter.foo = 1,
                    attr_1 = 10,
                    attr_2 = 20,
                    attr_3 = 30,
                    attr_4 = 40,
                    attr_5 = 50,
                    attr_6 = 60,
                    attr_7 = 70,
                    attr_8 = 80,
                    "msg"
                );
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("no_metrics_event_with_attributes_12", |b| {
            b.iter(|| {
                tracing::info!(
                    attr_1 = 10,
                    attr_2 = 20,
                    attr_3 = 30,
                    attr_4 = 40,
                    attr_5 = 50,
                    attr_6 = 60,
                    attr_7 = 70,
                    attr_8 = 80,
                    attr_9 = 90,
                    attr_10 = 100,
                    attr_11 = 110,
                    attr_12 = 120,
                    "msg"
                );
            })
        });
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = metrics_events
}
criterion_main!(benches);
