![Tracing — Structured, application-level diagnostics][splash]

[splash]: https://raw.githubusercontent.com/tokio-rs/tracing/master/assets/splash.svg

# Tracing OpenTelemetry

Utilities for adding [OpenTelemetry] interoperability to [`tracing`].

[![Crates.io][crates-badge]][crates-url]
[![Documentation][docs-badge]][docs-url]
[![Documentation (master)][docs-master-badge]][docs-master-url]
[![MIT licensed][mit-badge]][mit-url]
[![Build Status][actions-badge]][actions-url]
[![Discord chat][discord-badge]][discord-url]
![maintenance status][maint-badge]

[Documentation][docs-url] | [Chat][discord-url]

[crates-badge]: https://img.shields.io/crates/v/tracing-opentelemetry.svg
[crates-url]: https://crates.io/crates/tracing-opentelemetry/0.22.0
[docs-badge]: https://docs.rs/tracing-opentelemetry/badge.svg
[docs-url]: https://docs.rs/tracing-opentelemetry/0.22.0/tracing_opentelemetry
[docs-master-badge]: https://img.shields.io/badge/docs-master-blue
[docs-master-url]: https://tracing-rs.netlify.com/tracing_opentelemetry
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: LICENSE
[actions-badge]: https://github.com/tokio-rs/tracing-opentelemetry/workflows/CI/badge.svg
[actions-url]:https://github.com/tokio-rs/tracing-opentelemetry/actions?query=workflow%3ACI
[discord-badge]: https://img.shields.io/discord/500028886025895936?logo=discord&label=discord&logoColor=white
[discord-url]: https://discord.gg/EeF3cQw
[maint-badge]: https://img.shields.io/badge/maintenance-actively--developed-brightgreen.svg

## Overview

[`tracing`] is a framework for instrumenting Rust programs to collect
structured, event-based diagnostic information. This crate provides a
subscriber that connects spans from multiple systems into a trace and
emits them to [OpenTelemetry]-compatible distributed tracing systems
for processing and visualization.

The crate provides the following types:

* [`OpenTelemetryLayer`] adds OpenTelemetry context to all `tracing` [span]s.
* [`OpenTelemetrySpanExt`] allows OpenTelemetry parent trace information to be
  injected and extracted from a `tracing` [span].

[`OpenTelemetryLayer`]: https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/struct.OpenTelemetryLayer.html
[`OpenTelemetrySpanExt`]: https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/trait.OpenTelemetrySpanExt.html
[span]: https://docs.rs/tracing/latest/tracing/span/index.html
[`tracing`]: https://crates.io/crates/tracing
[OpenTelemetry]: https://opentelemetry.io/

## Compatibility with OpenTelemetry crates

Note that version numbers for this crate are **not** synchronized with the
various OpenTelemetry crates, despite having similar version numbers. For
discussion, see [issue #170](https://github.com/tokio-rs/tracing-opentelemetry/issues/170).

As of 0.26, tracing-opentelemetry is one version ahead of the opentelemetry
crates, such that tracing-opentelemetry 0.26.0 is compatible with opentelemetry 0.25.0,
but due to semver compatibility concerns, this may not always be the case.

## Visualizing traces with Jaeger

```console
# Run a supported collector like jaeger in the background
$ docker run -d -p4317:4317 -p16686:16686 jaegertracing/all-in-one:latest

# Run example to produce spans (from parent examples directory)
$ cargo run --example opentelemetry-otlp

# View spans (see the image below)
$ firefox http://localhost:16686/
```

![Jaeger UI](trace.png)

## Feature Flags

 - `metrics`: Enables the [`MetricsLayer`] type, a [layer] that
   exports OpenTelemetry metrics from specifically-named events. This enables
   the `metrics` feature flag on the `opentelemetry` crate.

[`MetricsLayer`]: https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/struct.MetricsLayer.html
[layer]: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html

## License

This project is licensed under the [MIT license](LICENSE).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in Tracing by you, shall be licensed as MIT, without any additional
terms or conditions.
