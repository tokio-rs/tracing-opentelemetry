use criterion::{criterion_group, criterion_main, Criterion};
use opentelemetry::{
    trace::{Span, SpanBuilder, Tracer as _, TracerProvider as _},
    Context,
};
use opentelemetry_sdk::trace::{Config, SpanLimits, Tracer, TracerProvider};
#[cfg(not(target_os = "windows"))]
use pprof::criterion::{Output, PProfProfiler};
use std::time::SystemTime;
use tracing::{trace, trace_span};
use tracing_subscriber::prelude::*;

fn many_children(c: &mut Criterion) {
    let mut group = c.benchmark_group("otel_many_children");

    group.bench_function("spec_baseline", |b| {
        let provider = TracerProvider::default();
        let tracer = provider.tracer("bench");
        b.iter(|| {
            fn dummy(tracer: &Tracer, cx: &Context) {
                for _ in 0..99 {
                    tracer.start_with_context("child", cx);
                }
            }

            tracer.in_span("parent", |cx| dummy(&tracer, &cx));
        });
    });

    {
        let _subscriber = tracing_subscriber::registry()
            .with(RegistryAccessLayer)
            .set_default();
        group.bench_function("no_data_baseline", |b| b.iter(tracing_harness));
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(OtelDataLayer)
            .set_default();
        group.bench_function("data_only_baseline", |b| b.iter(tracing_harness));
    }

    {
        let provider = TracerProvider::default();
        let tracer = provider.tracer("bench");
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_tracked_inactivity(false);
        let _subscriber = tracing_subscriber::registry()
            .with(otel_layer)
            .set_default();

        group.bench_function("full", |b| b.iter(tracing_harness));
    }
}

fn many_events(c: &mut Criterion) {
    let mut group = c.benchmark_group("otel_many_events");

    group.bench_function("spec_baseline", |b| {
        let provider = TracerProvider::default();
        let tracer = provider.tracer("bench");
        b.iter(|| {
            fn dummy(tracer: &Tracer, cx: &Context) {
                let mut span = tracer.start_with_context("child", cx);
                for _ in 0..1000 {
                    span.add_event("name", Vec::new());
                }
            }

            tracer.in_span("parent", |cx| dummy(&tracer, &cx));
        });
    });

    {
        let _subscriber = tracing_subscriber::registry()
            .with(RegistryAccessLayer)
            .set_default();
        group.bench_function("no_data_baseline", |b| b.iter(events_harness));
    }

    {
        let _subscriber = tracing_subscriber::registry()
            .with(OtelDataLayer)
            .set_default();
        group.bench_function("data_only_baseline", |b| b.iter(events_harness));
    }

    {
        let provider = TracerProvider::default();
        let tracer = provider.tracer("bench");
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_tracked_inactivity(false);
        let _subscriber = tracing_subscriber::registry()
            .with(otel_layer)
            .set_default();

        group.bench_function("full_filtered", |b| b.iter(events_harness));
    }

    {
        let provider = TracerProvider::builder()
            .with_config(Config {
                span_limits: SpanLimits {
                    max_events_per_span: 1000,
                    ..SpanLimits::default()
                },
                ..Config::default()
            })
            .build();
        let tracer = provider.tracer("bench");
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_tracked_inactivity(false);
        let _subscriber = tracing_subscriber::registry()
            .with(otel_layer)
            .set_default();

        group.bench_function("full_not_filtered", |b| b.iter(events_harness));
    }
}

struct NoDataSpan;
struct RegistryAccessLayer;

impl<S> tracing_subscriber::Layer<S> for RegistryAccessLayer
where
    S: tracing_core::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    fn on_new_span(
        &self,
        _attrs: &tracing_core::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();
        extensions.insert(NoDataSpan);
    }

    fn on_event(
        &self,
        event: &tracing_core::Event<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let Some(parent) = event.parent().and_then(|id| ctx.span(id)).or_else(|| {
            event
                .is_contextual()
                .then(|| ctx.lookup_current())
                .flatten()
        }) else {
            return;
        };
        let mut extensions = parent.extensions_mut();
        extensions.get_mut::<NoDataSpan>();
    }

    fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        extensions.remove::<NoDataSpan>();
    }
}

struct OtelDataLayer;

impl<S> tracing_subscriber::Layer<S> for OtelDataLayer
where
    S: tracing_core::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    fn on_new_span(
        &self,
        attrs: &tracing_core::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();
        extensions.insert(
            SpanBuilder::from_name(attrs.metadata().name()).with_start_time(SystemTime::now()),
        );
    }

    fn on_event(
        &self,
        event: &tracing_core::Event<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let Some(parent) = event.parent().and_then(|id| ctx.span(id)).or_else(|| {
            event
                .is_contextual()
                .then(|| ctx.lookup_current())
                .flatten()
        }) else {
            return;
        };
        let mut extensions = parent.extensions_mut();
        let builder = extensions
            .get_mut::<SpanBuilder>()
            .expect("Builder not found in span, this is a bug");
        let events = builder.events.get_or_insert_with(Vec::new);
        let otel_event =
            opentelemetry::trace::Event::new(String::new(), SystemTime::now(), Vec::new(), 0);
        events.push(otel_event);
    }

    fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");
        let mut extensions = span.extensions_mut();

        if let Some(builder) = extensions.remove::<SpanBuilder>() {
            builder.with_end_time(SystemTime::now());
        }
    }
}

fn tracing_harness() {
    fn dummy() {
        for _ in 0..99 {
            let child = trace_span!("child");
            let _enter = child.enter();
        }
    }

    let parent = trace_span!("parent");
    let _enter = parent.enter();

    dummy();
}

fn events_harness() {
    fn dummy() {
        let _child = trace_span!("child").entered();
        for _ in 0..1000 {
            trace!("event");
        }
    }

    let parent = trace_span!("parent");
    let _enter = parent.enter();

    dummy();
}

#[cfg(not(target_os = "windows"))]
criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = many_children, many_events
}
#[cfg(target_os = "windows")]
criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = many_children, many_events
}
criterion_main!(benches);
