//! `relay spec`: offline OpenAPI / JSON-schema output and typed-client
//! generation (`spec gen`).

use std::path::PathBuf;

use anyhow::Result;
use clap::{Subcommand, ValueEnum};

/// `relay spec [--format ...]` or `relay spec gen ...`. Positional slots are
/// reserved for the `gen` subcommand; everything else is a flag (the CLI
/// convention). relay has no request-shape cookbook / value-type catalog, so
/// keep's `--shapes`/`--fields` are deliberately absent (never faked).
#[derive(clap::Args, Debug)]
pub(crate) struct SpecArgs {
    /// Generate a typed client from the spec instead of printing it.
    #[command(subcommand)]
    pub(crate) gen: Option<SpecSub>,
    /// Schema format to emit.
    #[arg(long, value_enum, default_value_t = SpecFormat::Openapi)]
    pub(crate) format: SpecFormat,
}

#[derive(Subcommand, Debug)]
pub(crate) enum SpecSub {
    /// Generate a typed API client (TypeScript / Python / Rust) from relay's
    /// OpenAPI document, written into `--out`.
    Gen(GenArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct GenArgs {
    /// Target language for the generated client.
    #[arg(long, value_enum)]
    pub(crate) lang: GenLang,
    /// Pinned generated-client contract, e.g. `python-3.14`. Defaults to
    /// `clients/codegen.toml`; an explicit value overrides that policy once.
    #[arg(long, value_name = "TARGET")]
    pub(crate) target: Option<String>,
    /// Output directory for the generated files.
    #[arg(long)]
    pub(crate) out: PathBuf,
    /// HTTP backend for the TypeScript client (ignored for py/rust).
    #[arg(long, value_enum, default_value_t = GenHttp::Fetch)]
    pub(crate) http: GenHttp,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum GenLang {
    /// TypeScript: types + fetch/axios client + TanStack Query hooks.
    Ts,
    /// Python: pydantic models + a generated sync/async HTTP/2 runtime.
    Py,
    /// Rust: serde models + a reqwest client.
    Rust,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum GenHttp {
    Fetch,
    Axios,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum SpecFormat {
    /// Full OpenAPI 3 document as JSON (default).
    Openapi,
    /// Full OpenAPI 3 document as YAML for LLM/agent reading.
    #[value(alias = "yaml", alias = "openapi.yaml")]
    OpenapiYaml,
    /// Just the component schemas (honest: relay registers no named schemas
    /// today, so this serializes a null `components`).
    JsonSchema,
}

/// `relay spec` — offline OpenAPI (JSON / YAML / component schemas), or
/// `spec gen` to generate a typed client. No engine, no server, no I/O beyond
/// stdout / `--out` (#1209, keep #777).
pub(crate) fn run(args: SpecArgs) -> Result<()> {
    // `spec gen` writes a typed client; everything else prints to stdout.
    if let Some(SpecSub::Gen(gen)) = args.gen {
        return spec_gen(gen);
    }
    let out = match args.format {
        SpecFormat::Openapi => relay::transport::openapi::api_doc_json(),
        SpecFormat::OpenapiYaml => relay::transport::openapi::openapi_yaml(),
        SpecFormat::JsonSchema => relay::transport::openapi::json_schema_json(),
    };
    println!("{out}");
    Ok(())
}

/// `relay spec gen` — generate a typed client from relay's own OpenAPI
/// document (offline; no engine or server) via the shared
/// `core/openapi-codegen`, written into `--out`. One codegen path, no
/// external tool (keep's spec_gen verbatim).
fn spec_gen(args: GenArgs) -> Result<()> {
    use openapi_codegen::{
        generate_for_target, GenOptions, HttpClient, Lang, TargetPolicy, MANIFEST_FILE,
    };

    const TARGET_POLICY: &str = include_str!("../../../clients/codegen.toml");

    let lang = match args.lang {
        GenLang::Ts => Lang::Ts,
        GenLang::Py => Lang::Py,
        GenLang::Rust => Lang::Rust,
    };
    let target = TargetPolicy::from_toml(TARGET_POLICY)?.resolve(lang, args.target.as_deref())?;
    let opts = GenOptions {
        lang,
        target: Some(target),
        spec_path: PathBuf::new(),
        out_dir: args.out.clone(),
        client_name: "createClient".to_string(),
        http_client: match args.http {
            GenHttp::Fetch => HttpClient::Fetch,
            GenHttp::Axios => HttpClient::Axios,
        },
        emit_types: true,
        emit_client: true,
        // TanStack Query hooks are a TypeScript-only concern.
        emit_hooks: matches!(lang, Lang::Ts),
    };
    let output = generate_for_target(&relay::transport::openapi::api_doc_json(), &opts, target)?;
    output.write_to_dir(&args.out)?;
    for file in &output.files {
        let path = args.out.join(&file.rel_path);
        println!("generated {}", path.display());
    }
    println!("generated {}", args.out.join(MANIFEST_FILE).display());
    let requirements = output.requirements.expect("explicit target requirements");
    println!(
        "target: {} (minimum {} {})",
        requirements.target,
        requirements.language.id(),
        requirements.minimum_version
    );
    println!("next: done");
    Ok(())
}
