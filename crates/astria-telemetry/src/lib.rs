//! Initialize telemetry in all astria services.
//!
//! # Examples
//! ```no_run
//! astria_telemetry::configure()
//!     .filter_directives("info")
//!     .try_init()
//!     .expect("must be able to initialize telemetry");
//! tracing::info!("telemetry initialized");
//! ```
use std::io::IsTerminal as _;

use opentelemetry::{
    global,
    trace::TracerProvider as _,
};
use opentelemetry_sdk::{
    runtime::Tokio,
    trace::TracerProvider,
};
use opentelemetry_stdout::SpanExporter;
use tracing_subscriber::{
    filter::{
        LevelFilter,
        ParseError,
    },
    layer::SubscriberExt as _,
    util::{
        SubscriberInitExt as _,
        TryInitError,
    },
    EnvFilter,
};

#[cfg(feature = "display")]
pub mod display;

/// The errors that can occur when initializing telemtry.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct Error(ErrorKind);

impl Error {
    fn otlp(source: opentelemetry::trace::TraceError) -> Self {
        Self(ErrorKind::Otlp(source))
    }

    fn filter_directives(source: ParseError) -> Self {
        Self(ErrorKind::FilterDirectives(source))
    }

    fn init_subscriber(source: TryInitError) -> Self {
        Self(ErrorKind::InitSubscriber(source))
    }
}

#[derive(Debug, thiserror::Error)]
enum ErrorKind {
    #[error("failed constructing opentelemetry otlp exporter")]
    Otlp(#[source] opentelemetry::trace::TraceError),
    #[error("failed to parse filter directives")]
    FilterDirectives(#[source] ParseError),
    #[error("failed installing global tracing subscriber")]
    InitSubscriber(#[source] TryInitError),
}

#[must_use = "the otel config must be initialized to be useful"]
pub fn configure() -> Config {
    Config::new()
}
struct BoxedMakeWriter(Box<dyn MakeWriter + Send + Sync + 'static>);

impl BoxedMakeWriter {
    fn new<M>(make_writer: M) -> Self
    where
        M: MakeWriter + Send + Sync + 'static,
    {
        Self(Box::new(make_writer))
    }
}

pub trait MakeWriter {
    fn make_writer(&self) -> Box<dyn std::io::Write + Send + Sync + 'static>;
}

impl<F, W> MakeWriter for F
where
    F: Fn() -> W,
    W: std::io::Write + Send + Sync + 'static,
{
    fn make_writer(&self) -> Box<dyn std::io::Write + Send + Sync + 'static> {
        Box::new((self)())
    }
}

impl MakeWriter for BoxedMakeWriter {
    fn make_writer(&self) -> Box<dyn std::io::Write + Send + Sync + 'static> {
        self.0.make_writer()
    }
}

pub struct Config {
    filter_directives: String,
    force_stdout: bool,
    no_otel: bool,
    stdout_writer: BoxedMakeWriter,
}

impl Config {
    #[must_use = "telemetry must be initialized to be useful"]
    fn new() -> Self {
        Self {
            filter_directives: String::new(),
            force_stdout: false,
            no_otel: false,
            stdout_writer: BoxedMakeWriter::new(std::io::stdout),
        }
    }
}

impl Config {
    #[must_use = "telemetry must be initialized to be useful"]
    pub fn filter_directives(self, filter_directives: &str) -> Self {
        Self {
            filter_directives: filter_directives.to_string(),
            ..self
        }
    }

    #[must_use = "telemetry must be initialized to be useful"]
    pub fn force_stdout(self) -> Self {
        self.set_force_stdout(true)
    }

    #[must_use = "telemetry must be initialized to be useful"]
    pub fn set_force_stdout(self, force_stdout: bool) -> Self {
        Self {
            force_stdout,
            ..self
        }
    }

    #[must_use = "telemetry must be initialized to be useful"]
    pub fn no_otel(self) -> Self {
        self.set_no_otel(true)
    }

    #[must_use = "telemetry must be initialized to be useful"]
    pub fn set_no_otel(self, no_otel: bool) -> Self {
        Self {
            no_otel,
            ..self
        }
    }

    #[must_use = "telemetry must be initialized to be useful"]
    pub fn stdout_writer<M>(self, stdout_writer: M) -> Self
    where
        M: MakeWriter + Send + Sync + 'static,
    {
        Self {
            stdout_writer: BoxedMakeWriter::new(stdout_writer),
            ..self
        }
    }

    /// Initialize telemetry, consuming the config.
    ///
    /// # Errors
    /// Fails if the filter directives could not be parsed, if communication with the OTLP
    /// endpoint failed, or if the global tracing subscriber could not be installed.
    pub fn try_init(self) -> Result<(), Error> {
        let Self {
            filter_directives,
            force_stdout,
            no_otel,
            stdout_writer,
        } = self;

        let env_filter = {
            let builder = EnvFilter::builder().with_default_directive(LevelFilter::INFO.into());
            builder
                .parse(filter_directives)
                .map_err(Error::filter_directives)?
        };

        let mut tracer_provider = TracerProvider::builder();
        if !no_otel {
            // XXX: the endpoint is set by the env var OTEL_EXPORTER_OTLP_TRACES_ENDPOINT;
            //      this is hardcoded in OTEL.
            let otel_exporter = opentelemetry_otlp::new_exporter()
                .tonic()
                .build_span_exporter()
                .map_err(Error::otlp)?;
            tracer_provider = tracer_provider.with_batch_exporter(otel_exporter, Tokio);
        }

        if force_stdout || std::io::stdout().is_terminal() {
            tracer_provider = tracer_provider.with_simple_exporter(
                SpanExporter::builder()
                    .with_writer(stdout_writer.make_writer())
                    .build(),
            );
        }
        let tracer_provider = tracer_provider.build();

        let tracer = tracer_provider.versioned_tracer(
            "astria-telemetry",
            Some(env!("CARGO_PKG_VERSION")),
            Some(opentelemetry_semantic_conventions::SCHEMA_URL),
            None,
        );
        let _ = global::set_tracer_provider(tracer_provider);

        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
        tracing_subscriber::registry()
            .with(otel_layer)
            .with(env_filter)
            .try_init()
            .map_err(Error::init_subscriber)?;

        Ok(())
    }
}
