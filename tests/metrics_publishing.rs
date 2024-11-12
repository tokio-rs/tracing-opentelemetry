use opentelemetry::KeyValue;
use opentelemetry_sdk::{
    metrics::{
        data::{self, Gauge, Histogram, Sum},
        reader::MetricReader,
        InstrumentKind, ManualReader, MeterProviderBuilder, MetricError, SdkMeterProvider,
    },
    Resource,
};

use std::{fmt::Debug, sync::Arc};
use tracing::Subscriber;
use tracing_opentelemetry::MetricsLayer;
use tracing_subscriber::prelude::*;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTRUMENTATION_LIBRARY_NAME: &str = "tracing/tracing-opentelemetry";

#[tokio::test]
async fn u64_counter_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Counter,
        1_u64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(monotonic_counter.hello_world = 1_u64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn u64_counter_is_exported_i64_at_instrumentation_point() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world2".to_string(),
        InstrumentKind::Counter,
        1_u64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(monotonic_counter.hello_world2 = 1_i64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_counter_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "float_hello_world".to_string(),
        InstrumentKind::Counter,
        1.000000123_f64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(monotonic_counter.float_hello_world = 1.000000123_f64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn i64_up_down_counter_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "pebcak".to_string(),
        InstrumentKind::UpDownCounter,
        -5_i64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(counter.pebcak = -5_i64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn i64_up_down_counter_is_exported_u64_at_instrumentation_point() {
    let (subscriber, exporter) = init_subscriber(
        "pebcak2".to_string(),
        InstrumentKind::UpDownCounter,
        5_i64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(counter.pebcak2 = 5_u64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_up_down_counter_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "pebcak_blah".to_string(),
        InstrumentKind::UpDownCounter,
        99.123_f64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(counter.pebcak_blah = 99.123_f64);
    });

    exporter.export().unwrap();
}

#[cfg(feature = "metrics_gauge_unstable")]
#[tokio::test]
async fn u64_gauge_is_exported() {
    let (subscriber, exporter) =
        init_subscriber("gygygy".to_string(), InstrumentKind::Gauge, 2_u64, None);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(gauge.gygygy = 1_u64);
        tracing::info!(gauge.gygygy = 2_u64);
    });

    exporter.export().unwrap();
}

#[cfg(feature = "metrics_gauge_unstable")]
#[tokio::test]
async fn f64_gauge_is_exported() {
    let (subscriber, exporter) =
        init_subscriber("huitt".to_string(), InstrumentKind::Gauge, 2_f64, None);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(gauge.huitt = 1_f64);
        tracing::info!(gauge.huitt = 2_f64);
    });

    exporter.export().unwrap();
}

#[cfg(feature = "metrics_gauge_unstable")]
#[tokio::test]
async fn i64_gauge_is_exported() {
    let (subscriber, exporter) =
        init_subscriber("samsagaz".to_string(), InstrumentKind::Gauge, 2_i64, None);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(gauge.samsagaz = 1_i64);
        tracing::info!(gauge.samsagaz = 2_i64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn u64_histogram_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "abcdefg".to_string(),
        InstrumentKind::Histogram,
        9_u64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(histogram.abcdefg = 9_u64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_histogram_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "abcdefg_racecar".to_string(),
        InstrumentKind::Histogram,
        777.0012_f64,
        None,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(histogram.abcdefg_racecar = 777.0012_f64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn u64_counter_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Counter,
        1_u64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            monotonic_counter.hello_world = 1_u64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_counter_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Counter,
        1_f64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            monotonic_counter.hello_world = 1_f64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn i64_up_down_counter_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::UpDownCounter,
        -1_i64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            counter.hello_world = -1_i64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_up_down_counter_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::UpDownCounter,
        -1_f64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            counter.hello_world = -1_f64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[cfg(feature = "metrics_gauge_unstable")]
#[tokio::test]
async fn f64_gauge_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Gauge,
        1_f64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            gauge.hello_world = 1_f64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[cfg(feature = "metrics_gauge_unstable")]
#[tokio::test]
async fn u64_gauge_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Gauge,
        1_u64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            gauge.hello_world = 1_u64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[cfg(feature = "metrics_gauge_unstable")]
#[tokio::test]
async fn i64_gauge_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Gauge,
        1_i64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            gauge.hello_world = 1_i64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn u64_histogram_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Histogram,
        1_u64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            histogram.hello_world = 1_u64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_histogram_with_attributes_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Histogram,
        1_f64,
        Some(vec![
            KeyValue::new("u64_key_1", 1_i64),
            KeyValue::new("i64_key_1", 2_i64),
            KeyValue::new("f64_key_1", 3_f64),
            KeyValue::new("str_key_1", "foo"),
            KeyValue::new("bool_key_1", true),
        ]),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            histogram.hello_world = 1_f64,
            u64_key_1 = 1_u64,
            i64_key_1 = 2_i64,
            f64_key_1 = 3_f64,
            str_key_1 = "foo",
            bool_key_1 = true,
        );
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn display_attribute_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Counter,
        1_u64,
        Some(vec![KeyValue::new("display_key_1", "display: foo")]),
    );

    struct DisplayAttribute(String);

    impl std::fmt::Display for DisplayAttribute {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "display: {}", self.0)
        }
    }

    let display_attribute = DisplayAttribute("foo".to_string());

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            monotonic_counter.hello_world = 1_u64,
            display_key_1 = %display_attribute,
        );
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn debug_attribute_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "hello_world".to_string(),
        InstrumentKind::Counter,
        1_u64,
        Some(vec![KeyValue::new("debug_key_1", "debug: foo")]),
    );

    struct DebugAttribute(String);

    impl std::fmt::Debug for DebugAttribute {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "debug: {}", self.0)
        }
    }

    let debug_attribute = DebugAttribute("foo".to_string());

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            monotonic_counter.hello_world = 1_u64,
            debug_key_1 = ?debug_attribute,
        );
    });

    exporter.export().unwrap();
}

fn init_subscriber<T>(
    expected_metric_name: String,
    expected_instrument_kind: InstrumentKind,
    expected_value: T,
    expected_attributes: Option<Vec<KeyValue>>,
) -> (impl Subscriber + 'static, TestExporter<T>) {
    let reader = ManualReader::builder().build();
    let reader = TestReader {
        inner: Arc::new(reader),
    };

    let provider = MeterProviderBuilder::default()
        .with_reader(reader.clone())
        .build();
    let exporter = TestExporter {
        expected_metric_name,
        expected_instrument_kind,
        expected_value,
        expected_attributes,
        reader,
        _meter_provider: provider.clone(),
    };

    (
        tracing_subscriber::registry().with(MetricsLayer::new(provider)),
        exporter,
    )
}

#[derive(Debug, Clone)]
struct TestReader {
    inner: Arc<ManualReader>,
}

impl MetricReader for TestReader {
    fn register_pipeline(&self, pipeline: std::sync::Weak<opentelemetry_sdk::metrics::Pipeline>) {
        self.inner.register_pipeline(pipeline);
    }

    fn collect(
        &self,
        rm: &mut data::ResourceMetrics,
    ) -> opentelemetry_sdk::metrics::MetricResult<()> {
        self.inner.collect(rm)
    }

    fn force_flush(&self) -> opentelemetry_sdk::metrics::MetricResult<()> {
        self.inner.force_flush()
    }

    fn shutdown(&self) -> opentelemetry_sdk::metrics::MetricResult<()> {
        self.inner.shutdown()
    }

    fn temporality(&self, kind: InstrumentKind) -> opentelemetry_sdk::metrics::Temporality {
        self.inner.temporality(kind)
    }
}

struct TestExporter<T> {
    expected_metric_name: String,
    expected_instrument_kind: InstrumentKind,
    expected_value: T,
    expected_attributes: Option<Vec<KeyValue>>,
    reader: TestReader,
    _meter_provider: SdkMeterProvider,
}

impl<T> TestExporter<T>
where
    T: Debug + PartialEq + Copy + std::iter::Sum + 'static,
{
    fn export(&self) -> Result<(), MetricError> {
        let mut rm = data::ResourceMetrics {
            resource: Resource::default(),
            scope_metrics: Vec::new(),
        };
        self.reader.collect(&mut rm)?;

        assert!(!rm.scope_metrics.is_empty());

        rm.scope_metrics.into_iter().for_each(|scope_metrics| {
            assert_eq!(scope_metrics.scope.name(), INSTRUMENTATION_LIBRARY_NAME);
            assert_eq!(scope_metrics.scope.version().unwrap(), CARGO_PKG_VERSION);

            scope_metrics.metrics.into_iter().for_each(|metric| {
                assert_eq!(metric.name, self.expected_metric_name);

                match self.expected_instrument_kind {
                    InstrumentKind::Counter | InstrumentKind::UpDownCounter => {
                        let sum = metric.data.as_any().downcast_ref::<Sum<T>>().unwrap();
                        assert_eq!(
                            self.expected_value,
                            sum.data_points
                                .iter()
                                .map(|data_point| data_point.value)
                                .sum()
                        );

                        if let Some(expected_attributes) = self.expected_attributes.as_ref() {
                            sum.data_points.iter().for_each(|data_point| {
                                assert!(compare_attributes(
                                    expected_attributes,
                                    &data_point.attributes,
                                ))
                            });
                        }
                    }
                    InstrumentKind::Gauge => {
                        let gauge = metric.data.as_any().downcast_ref::<Gauge<T>>().unwrap();
                        assert_eq!(
                            self.expected_value,
                            gauge
                                .data_points
                                .iter()
                                .map(|data_point| data_point.value)
                                .last()
                                .unwrap()
                        );

                        if let Some(expected_attributes) = self.expected_attributes.as_ref() {
                            gauge.data_points.iter().for_each(|data_point| {
                                assert!(compare_attributes(
                                    expected_attributes,
                                    &data_point.attributes,
                                ))
                            });
                        }
                    }
                    InstrumentKind::Histogram => {
                        let histogram =
                            metric.data.as_any().downcast_ref::<Histogram<T>>().unwrap();
                        let histogram_data = histogram.data_points.first().unwrap();
                        assert!(histogram_data.count > 0);
                        assert_eq!(histogram_data.sum, self.expected_value);

                        if let Some(expected_attributes) = self.expected_attributes.as_ref() {
                            assert!(compare_attributes(
                                expected_attributes,
                                &histogram_data.attributes
                            ))
                        }
                    }
                    unexpected => {
                        panic!("InstrumentKind {:?} not currently supported!", unexpected)
                    }
                }
            });
        });

        Ok(())
    }
}

// After sorting the KeyValue vec, compare them.
// Return true if they are equal.
#[allow(clippy::ptr_arg)]
fn compare_attributes(expected: &Vec<KeyValue>, actual: &Vec<KeyValue>) -> bool {
    let mut expected = expected.clone();
    let mut actual = actual.clone();

    expected.sort_unstable_by(|a, b| a.key.cmp(&b.key));
    actual.sort_unstable_by(|a, b| a.key.cmp(&b.key));

    expected == actual
}
