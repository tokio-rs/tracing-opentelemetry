use criterion::{criterion_group, criterion_main, Criterion};
use opentelemetry::metrics::noop::NoopMeterProvider;
#[cfg(not(target_os = "windows"))]
use pprof::criterion::{Output, PProfProfiler};
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
        group.bench_function("metrics_events_0_attr_0", |b| {
            b.iter(|| {
                tracing::info!(key_1 = "va", "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_1_attr_0", |b| {
            b.iter(|| {
                tracing::info!(monotonic_counter.c1 = 1, "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_2_attr_0", |b| {
            b.iter(|| {
                tracing::info!(monotonic_counter.c1 = 1, monotonic_counter.c2 = 1, "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_4_attr_0", |b| {
            b.iter(|| {
                tracing::info!(
                    monotonic_counter.c1 = 1,
                    monotonic_counter.c2 = 1,
                    monotonic_counter.c3 = 1,
                    monotonic_counter.c4 = 1,
                    "msg"
                );
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_8_attr_0", |b| {
            b.iter(|| {
                tracing::info!(
                    monotonic_counter.c1 = 1,
                    monotonic_counter.c2 = 1,
                    monotonic_counter.c3 = 1,
                    monotonic_counter.c4 = 1,
                    monotonic_counter.c5 = 1,
                    monotonic_counter.c6 = 1,
                    monotonic_counter.c7 = 1,
                    monotonic_counter.c8 = 1,
                    "msg"
                );
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_1_attr_1", |b| {
            b.iter(|| {
                tracing::info!(monotonic_counter.c1 = 1, key_1 = 1_i64, "msg");
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_1_attr_2", |b| {
            b.iter(|| {
                tracing::info!(
                    monotonic_counter.c1 = 1,
                    key_1 = 1_i64,
                    key_2 = 1_i64,
                    "msg"
                );
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_1_attr_4", |b| {
            b.iter(|| {
                tracing::info!(
                    monotonic_counter.c1 = 1,
                    key_1 = 1_i64,
                    key_2 = 1_i64,
                    key_3 = 1_i64,
                    key_4 = 1_i64,
                    "msg"
                );
            })
        });
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(MetricsLayer::new(NoopMeterProvider::new()))
            .set_default();
        group.bench_function("metrics_events_1_attr_8", |b| {
            b.iter(|| {
                tracing::info!(
                    monotonic_counter.c1 = 1,
                    key_1 = 1_i64,
                    key_2 = 1_i64,
                    key_3 = 1_i64,
                    key_4 = 1_i64,
                    key_5 = 1_i64,
                    key_6 = 1_i64,
                    key_7 = 1_i64,
                    key_8 = 1_i64,
                    "msg"
                );
            })
        });
    }
    group.finish();
}

#[cfg(not(target_os = "windows"))]
criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = metrics_events
}
#[cfg(target_os = "windows")]
criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = metrics_events
}
criterion_main!(benches);
