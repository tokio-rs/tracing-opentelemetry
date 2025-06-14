use std::{collections::HashMap, fmt, sync::RwLock};
use tracing::{field::Visit, Subscriber};
use tracing_core::{Field, Interest, Metadata};

use opentelemetry::{
    metrics::{Counter, Gauge, Histogram, Meter, MeterProvider, UpDownCounter},
    InstrumentationScope, KeyValue, Value,
};
use tracing_subscriber::{
    filter::Filtered,
    layer::{Context, Filter},
    registry::LookupSpan,
    Layer,
};

use smallvec::SmallVec;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTRUMENTATION_LIBRARY_NAME: &str = "tracing/tracing-opentelemetry";

const METRIC_PREFIX_MONOTONIC_COUNTER: &str = "monotonic_counter.";
const METRIC_PREFIX_COUNTER: &str = "counter.";
const METRIC_PREFIX_HISTOGRAM: &str = "histogram.";
const METRIC_PREFIX_GAUGE: &str = "gauge.";

const I64_MAX: u64 = i64::MAX as u64;

#[derive(Default)]
pub(crate) struct Instruments {
    u64_counter: MetricsMap<Counter<u64>>,
    f64_counter: MetricsMap<Counter<f64>>,
    i64_up_down_counter: MetricsMap<UpDownCounter<i64>>,
    f64_up_down_counter: MetricsMap<UpDownCounter<f64>>,
    u64_histogram: MetricsMap<Histogram<u64>>,
    f64_histogram: MetricsMap<Histogram<f64>>,
    u64_gauge: MetricsMap<Gauge<u64>>,
    i64_gauge: MetricsMap<Gauge<i64>>,
    f64_gauge: MetricsMap<Gauge<f64>>,
}

type MetricsMap<T> = RwLock<HashMap<&'static str, T>>;

#[derive(Copy, Clone, Debug)]
pub(crate) enum InstrumentType {
    CounterU64(u64),
    CounterF64(f64),
    UpDownCounterI64(i64),
    UpDownCounterF64(f64),
    HistogramU64(u64),
    HistogramF64(f64),
    GaugeU64(u64),
    GaugeI64(i64),
    GaugeF64(f64),
}

impl Instruments {
    pub(crate) fn update_metric(
        &self,
        meter: &Meter,
        instrument_type: InstrumentType,
        metric_name: &'static str,
        attributes: &[KeyValue],
    ) {
        fn update_or_insert<T>(
            map: &MetricsMap<T>,
            name: &'static str,
            insert: impl FnOnce() -> T,
            update: impl FnOnce(&T),
        ) {
            {
                let lock = map.read().unwrap();
                if let Some(metric) = lock.get(name) {
                    update(metric);
                    return;
                }
            }

            // that metric did not already exist, so we have to acquire a write lock to
            // create it.
            let mut lock = map.write().unwrap();
            // handle the case where the entry was created while we were waiting to
            // acquire the write lock
            let metric = lock.entry(name).or_insert_with(insert);
            update(metric)
        }

        match instrument_type {
            InstrumentType::CounterU64(value) => {
                update_or_insert(
                    &self.u64_counter,
                    metric_name,
                    || meter.u64_counter(metric_name).build(),
                    |ctr| ctr.add(value, attributes),
                );
            }
            InstrumentType::CounterF64(value) => {
                update_or_insert(
                    &self.f64_counter,
                    metric_name,
                    || meter.f64_counter(metric_name).build(),
                    |ctr| ctr.add(value, attributes),
                );
            }
            InstrumentType::UpDownCounterI64(value) => {
                update_or_insert(
                    &self.i64_up_down_counter,
                    metric_name,
                    || meter.i64_up_down_counter(metric_name).build(),
                    |ctr| ctr.add(value, attributes),
                );
            }
            InstrumentType::UpDownCounterF64(value) => {
                update_or_insert(
                    &self.f64_up_down_counter,
                    metric_name,
                    || meter.f64_up_down_counter(metric_name).build(),
                    |ctr| ctr.add(value, attributes),
                );
            }
            InstrumentType::HistogramU64(value) => {
                update_or_insert(
                    &self.u64_histogram,
                    metric_name,
                    || meter.u64_histogram(metric_name).build(),
                    |rec| rec.record(value, attributes),
                );
            }
            InstrumentType::HistogramF64(value) => {
                update_or_insert(
                    &self.f64_histogram,
                    metric_name,
                    || meter.f64_histogram(metric_name).build(),
                    |rec| rec.record(value, attributes),
                );
            }
            InstrumentType::GaugeU64(value) => {
                update_or_insert(
                    &self.u64_gauge,
                    metric_name,
                    || meter.u64_gauge(metric_name).build(),
                    |rec| rec.record(value, attributes),
                );
            }
            InstrumentType::GaugeI64(value) => {
                update_or_insert(
                    &self.i64_gauge,
                    metric_name,
                    || meter.i64_gauge(metric_name).build(),
                    |rec| rec.record(value, attributes),
                );
            }
            InstrumentType::GaugeF64(value) => {
                update_or_insert(
                    &self.f64_gauge,
                    metric_name,
                    || meter.f64_gauge(metric_name).build(),
                    |rec| rec.record(value, attributes),
                );
            }
        };
    }
}

pub(crate) struct MetricVisitor<'a> {
    attributes: &'a mut SmallVec<[KeyValue; 8]>,
    visited_metrics: &'a mut SmallVec<[(&'static str, InstrumentType); 2]>,
}

impl Visit for MetricVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.attributes
            .push(KeyValue::new(field.name(), format!("{value:?}")));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_GAUGE) {
            self.visited_metrics
                .push((metric_name, InstrumentType::GaugeU64(value)));
            return;
        }
        if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_MONOTONIC_COUNTER) {
            self.visited_metrics
                .push((metric_name, InstrumentType::CounterU64(value)));
        } else if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_COUNTER) {
            if value <= I64_MAX {
                self.visited_metrics
                    .push((metric_name, InstrumentType::UpDownCounterI64(value as i64)));
            } else {
                eprintln!(
                    "[tracing-opentelemetry]: Received Counter metric, but \
                    provided u64: {} is greater than i64::MAX. Ignoring \
                    this metric.",
                    value
                );
            }
        } else if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_HISTOGRAM) {
            self.visited_metrics
                .push((metric_name, InstrumentType::HistogramU64(value)));
        } else if value <= I64_MAX {
            self.attributes
                .push(KeyValue::new(field.name(), Value::I64(value as i64)));
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_GAUGE) {
            self.visited_metrics
                .push((metric_name, InstrumentType::GaugeF64(value)));
            return;
        }
        if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_MONOTONIC_COUNTER) {
            self.visited_metrics
                .push((metric_name, InstrumentType::CounterF64(value)));
        } else if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_COUNTER) {
            self.visited_metrics
                .push((metric_name, InstrumentType::UpDownCounterF64(value)));
        } else if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_HISTOGRAM) {
            self.visited_metrics
                .push((metric_name, InstrumentType::HistogramF64(value)));
        } else {
            self.attributes
                .push(KeyValue::new(field.name(), Value::F64(value)));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_GAUGE) {
            self.visited_metrics
                .push((metric_name, InstrumentType::GaugeI64(value)));
            return;
        }
        if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_MONOTONIC_COUNTER) {
            self.visited_metrics
                .push((metric_name, InstrumentType::CounterU64(value as u64)));
        } else if let Some(metric_name) = field.name().strip_prefix(METRIC_PREFIX_COUNTER) {
            self.visited_metrics
                .push((metric_name, InstrumentType::UpDownCounterI64(value)));
        } else {
            self.attributes.push(KeyValue::new(field.name(), value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.attributes
            .push(KeyValue::new(field.name(), value.to_owned()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.attributes.push(KeyValue::new(field.name(), value));
    }
}

/// A layer that publishes metrics via the OpenTelemetry SDK.
///
/// # Usage
///
/// No configuration is needed for this Layer, as it's only responsible for
/// pushing data out to the `opentelemetry` family of crates. For example, when
/// using `opentelemetry-otlp`, that crate will provide its own set of
/// configuration options for setting up the duration metrics will be collected
/// before exporting to the OpenTelemetry Collector, aggregation of data points,
/// etc.
///
/// ```no_run
/// use tracing_opentelemetry::MetricsLayer;
/// use tracing_subscriber::layer::SubscriberExt;
/// use tracing_subscriber::Registry;
/// # use opentelemetry_sdk::metrics::SdkMeterProvider;
///
/// // Constructing a MeterProvider is out-of-scope for the docs here, but there
/// // are examples in the opentelemetry repository. See:
/// // https://github.com/open-telemetry/opentelemetry-rust/blob/dfeac078ff7853e7dc814778524b93470dfa5c9c/examples/metrics-basic/src/main.rs#L7
/// # let meter_provider: SdkMeterProvider = unimplemented!();
///
/// let opentelemetry_metrics =  MetricsLayer::new(meter_provider);
/// let subscriber = Registry::default().with(opentelemetry_metrics);
/// tracing::subscriber::set_global_default(subscriber).unwrap();
/// ```
///
/// To publish a new metric, add a key-value pair to your `tracing::Event` that
/// contains following prefixes:
/// - `monotonic_counter.` (non-negative numbers): Used when the counter should
///   only ever increase
/// - `counter.`: Used when the counter can go up or down
/// - `histogram.`: Used to report arbitrary values that are likely to be statistically meaningful
///
/// Examples:
/// ```
/// # use tracing::info;
/// info!(monotonic_counter.foo = 1);
/// info!(monotonic_counter.bar = 1.1);
///
/// info!(counter.baz = 1);
/// info!(counter.baz = -1);
/// info!(counter.xyz = 1.1);
///
/// info!(histogram.qux = 1);
/// info!(histogram.abc = -1);
/// info!(histogram.def = 1.1);
/// ```
///
/// # Mixing data types
///
/// ## Floating-point numbers
///
/// Do not mix floating point and non-floating point numbers for the same
/// metric. If a floating point number will be used for a given metric, be sure
/// to cast any other usages of that metric to a floating point number.
///
/// Do this:
/// ```
/// # use tracing::info;
/// info!(monotonic_counter.foo = 1_f64);
/// info!(monotonic_counter.foo = 1.1);
/// ```
///
/// This is because all data published for a given metric name must be the same
/// numeric type.
///
/// ## Integers
///
/// Positive and negative integers can be mixed freely. The instrumentation
/// provided by `tracing` assumes that all integers are `i64` unless explicitly
/// cast to something else. In the case that an integer *is* cast to `u64`, this
/// subscriber will handle the conversion internally.
///
/// For example:
/// ```
/// # use tracing::info;
/// // The subscriber receives an i64
/// info!(counter.baz = 1);
///
/// // The subscriber receives an i64
/// info!(counter.baz = -1);
///
/// // The subscriber receives a u64, but casts it to i64 internally
/// info!(counter.baz = 1_u64);
///
/// // The subscriber receives a u64, but cannot cast it to i64 because of
/// // overflow. An error is printed to stderr, and the metric is dropped.
/// info!(counter.baz = (i64::MAX as u64) + 1)
/// ```
///
/// # Attributes
///
/// When `MetricsLayer` outputs metrics, it converts key-value pairs into [Attributes] and associates them with metrics.
///
/// [Attributes]: https://opentelemetry.io/docs/specs/otel/common/#attribute
///
/// For example:
/// ```
/// # use tracing::info;
/// // adds attributes bar="baz" and qux=2 to the `foo` counter.
/// info!(monotonic_counter.foo = 1, bar = "baz", qux = 2);
/// ```
///
/// # Implementation Details
///
/// `MetricsLayer` holds a set of maps, with each map corresponding to a
/// type of metric supported by OpenTelemetry. These maps are populated lazily.
/// The first time that a metric is emitted by the instrumentation, a `Metric`
/// instance will be created and added to the corresponding map. This means that
/// any time a metric is emitted by the instrumentation, one map lookup has to
/// be performed.
///
/// In the future, this can be improved by associating each `Metric` instance to
/// its callsite, eliminating the need for any maps.
///
#[cfg_attr(docsrs, doc(cfg(feature = "metrics")))]
pub struct MetricsLayer<S> {
    inner: Filtered<InstrumentLayer, MetricsFilter, S>,
}

impl<S> MetricsLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    /// Create a new instance of MetricsLayer.
    pub fn new<M>(meter_provider: M) -> MetricsLayer<S>
    where
        M: MeterProvider,
    {
        let meter = meter_provider.meter_with_scope(
            InstrumentationScope::builder(INSTRUMENTATION_LIBRARY_NAME)
                .with_version(CARGO_PKG_VERSION)
                .build(),
        );

        let layer = InstrumentLayer {
            meter,
            instruments: Default::default(),
        };

        MetricsLayer {
            inner: layer.with_filter(MetricsFilter),
        }
    }
}

struct MetricsFilter;

impl MetricsFilter {
    fn is_metrics_event(&self, meta: &Metadata<'_>) -> bool {
        meta.is_event()
            && meta.fields().iter().any(|field| {
                let name = field.name();

                if name.starts_with(METRIC_PREFIX_COUNTER)
                    || name.starts_with(METRIC_PREFIX_MONOTONIC_COUNTER)
                    || name.starts_with(METRIC_PREFIX_HISTOGRAM)
                {
                    return true;
                }

                if name.starts_with(METRIC_PREFIX_GAUGE) {
                    return true;
                }

                false
            })
    }
}

impl<S> Filter<S> for MetricsFilter {
    fn enabled(&self, meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        self.is_metrics_event(meta)
    }

    fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
        if self.is_metrics_event(meta) {
            Interest::always()
        } else {
            Interest::never()
        }
    }
}

struct InstrumentLayer {
    meter: Meter,
    instruments: Instruments,
}

impl<S> Layer<S> for InstrumentLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut attributes = SmallVec::new();
        let mut visited_metrics = SmallVec::new();
        let mut metric_visitor = MetricVisitor {
            attributes: &mut attributes,
            visited_metrics: &mut visited_metrics,
        };
        event.record(&mut metric_visitor);

        // associate attrivutes with visited metrics
        visited_metrics
            .into_iter()
            .for_each(|(metric_name, value)| {
                self.instruments.update_metric(
                    &self.meter,
                    value,
                    metric_name,
                    attributes.as_slice(),
                );
            })
    }
}

impl<S> Layer<S> for MetricsLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_layer(&mut self, subscriber: &mut S) {
        self.inner.on_layer(subscriber)
    }

    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        self.inner.register_callsite(metadata)
    }

    fn enabled(&self, metadata: &Metadata<'_>, ctx: Context<'_, S>) -> bool {
        self.inner.enabled(metadata, ctx)
    }

    fn on_new_span(
        &self,
        attrs: &tracing_core::span::Attributes<'_>,
        id: &tracing_core::span::Id,
        ctx: Context<'_, S>,
    ) {
        self.inner.on_new_span(attrs, id, ctx)
    }

    fn max_level_hint(&self) -> Option<tracing_core::LevelFilter> {
        self.inner.max_level_hint()
    }

    fn on_record(
        &self,
        span: &tracing_core::span::Id,
        values: &tracing_core::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        self.inner.on_record(span, values, ctx)
    }

    fn on_follows_from(
        &self,
        span: &tracing_core::span::Id,
        follows: &tracing_core::span::Id,
        ctx: Context<'_, S>,
    ) {
        self.inner.on_follows_from(span, follows, ctx)
    }

    fn on_event(&self, event: &tracing_core::Event<'_>, ctx: Context<'_, S>) {
        self.inner.on_event(event, ctx)
    }

    fn on_enter(&self, id: &tracing_core::span::Id, ctx: Context<'_, S>) {
        self.inner.on_enter(id, ctx)
    }

    fn on_exit(&self, id: &tracing_core::span::Id, ctx: Context<'_, S>) {
        self.inner.on_exit(id, ctx)
    }

    fn on_close(&self, id: tracing_core::span::Id, ctx: Context<'_, S>) {
        self.inner.on_close(id, ctx)
    }

    fn on_id_change(
        &self,
        old: &tracing_core::span::Id,
        new: &tracing_core::span::Id,
        ctx: Context<'_, S>,
    ) {
        self.inner.on_id_change(old, new, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    struct PanicLayer;
    impl<S> Layer<S> for PanicLayer
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        fn on_event(&self, _event: &tracing_core::Event<'_>, _ctx: Context<'_, S>) {
            panic!("panic");
        }
    }

    #[test]
    fn filter_layer_should_filter_non_metrics_event() {
        let layer = PanicLayer.with_filter(MetricsFilter);
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(key = "val", "foo");
        });
    }
}
