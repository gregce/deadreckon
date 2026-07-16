use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;
use which::which;

use crate::cli_common::{CliRunOptions, ensure_success, run_cli_with_options, write_output};
use crate::cli_contract::{
    ProviderContract, add_caveat, flight_rows_from, probe_descriptor_contract,
};
use crate::registry::ProviderDescriptor;
use crate::{
    Provider, ProviderEntry, ProviderError, ProviderFuture, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderUsage, Result, SpendEstimate,
};

#[derive(Clone)]
pub(crate) struct GenericCliProvider {
    name: String,
    descriptor: ProviderDescriptor,
    binary: String,
    extra_args: Vec<String>,
    model: String,
    model_arg: Option<String>,
    contract: Option<ProviderContract>,
}

impl GenericCliProvider {
    pub(crate) fn new(
        name: impl Into<String>,
        entry: ProviderEntry,
        descriptor: ProviderDescriptor,
    ) -> Result<Self> {
        if descriptor.exec_template.args_template.is_empty() {
            return Err(ProviderError::InvalidConfig(format!(
                "{} cli descriptor missing exec_template.args_template",
                descriptor.id
            )));
        }
        let (model, model_arg) = cli_model(entry.model, &descriptor);
        let contract = descriptor
            .contract
            .as_ref()
            .map(ProviderContract::from_descriptor)
            .transpose()
            .map_err(|error| {
                ProviderError::InvalidConfig(format!(
                    "{} descriptor contract is invalid: {error}",
                    descriptor.id
                ))
            })?;
        let binary = entry
            .binary
            .or_else(|| descriptor.default_binary.clone())
            .unwrap_or_else(|| descriptor.id.clone());
        Ok(Self {
            name: name.into(),
            descriptor,
            binary,
            extra_args: entry.extra_args,
            model,
            model_arg,
            contract,
        })
    }

    async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        match self.contract.as_ref() {
            Some(contract) => self.run_contract(request, contract).await,
            None => self.run_contractless(request).await,
        }
    }

    /// The pre-Pennant runner, deliberately kept as an intact branch so a
    /// descriptor without `[contract]` remains byte-for-byte compatible.
    async fn run_contractless(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let started = Instant::now();
        let args = self.render_args(request);
        let sandbox_writes = self.sandbox_writes();
        let output = run_cli_with_options(
            &self.name,
            &self.binary,
            &args,
            CliRunOptions {
                cwd: request.cwd.clone(),
                sandbox_backend: request.sandbox_backend,
                pid_file: request.pid_file.clone(),
                cancellation_token: request.cancellation_token.clone(),
                extra_write_allowlist: sandbox_writes.clone(),
            },
        )
        .await?;
        write_output(request.output_path.as_ref(), &output).await?;
        ensure_success(&self.name, &output)?;
        let wall_time_seconds = started.elapsed().as_secs_f64();
        let usage = ProviderUsage {
            input_tokens: 0,
            output_tokens: 0,
        };
        let spend = self
            .estimate_spend(usage.clone())
            .with_wall_time(wall_time_seconds);
        Ok(ProviderResponse {
            provider: self.name.clone(),
            model: self.model.clone(),
            content: output.stdout.clone(),
            usage,
            spend,
            trace: json!({
                "kind": "cli_subagent",
                "binary": self.binary,
                "args": args,
                "stdout_path": request.output_path,
                "duration_ms": (wall_time_seconds * 1000.0).round() as u64,
                "exit_code": output.status_code,
                "pid": output.pid,
                "sandbox_backend": output.sandbox_backend,
                "sandbox_warning": output.sandbox_warning,
                "descriptor": self.descriptor.id,
                "sandbox_write_allowlist": sandbox_writes,
            }),
        })
    }

    async fn run_contract(
        &self,
        request: &ProviderRequest,
        contract: &ProviderContract,
    ) -> Result<ProviderResponse> {
        let probe = probe_descriptor_contract(&self.binary, contract);
        let started = Instant::now();
        let args = if probe.active {
            self.render_contract_args(request, contract)
        } else {
            self.render_args(request)
        };
        let sandbox_writes = self.sandbox_writes();
        let output = run_cli_with_options(
            &self.name,
            &self.binary,
            &args,
            CliRunOptions {
                cwd: request.cwd.clone(),
                sandbox_backend: request.sandbox_backend,
                pid_file: request.pid_file.clone(),
                cancellation_token: request.cancellation_token.clone(),
                extra_write_allowlist: sandbox_writes.clone(),
            },
        )
        .await?;
        write_output(request.output_path.as_ref(), &output).await?;
        ensure_success(&self.name, &output)?;

        let extracted = probe.active.then(|| contract.parse(&output.stdout));
        if let Some(message) = extracted
            .as_ref()
            .and_then(|extracted| extracted.parsed.failure.as_ref())
        {
            return Err(ProviderError::Cli {
                provider: self.name.clone(),
                detail: format!("provider contract reported an error: {message}"),
            });
        }
        let degraded = extracted
            .as_ref()
            .is_some_and(|extracted| extracted.parsed.degraded());
        let content = extracted
            .as_ref()
            .filter(|_| !degraded)
            .and_then(|extracted| extracted.parsed.answer.clone())
            .unwrap_or_else(|| output.stdout.clone());
        let usage = extracted
            .as_ref()
            .and_then(|extracted| extracted.parsed.usage)
            .map(|usage| ProviderUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
            })
            .unwrap_or(ProviderUsage {
                input_tokens: 0,
                output_tokens: 0,
            });
        let reported_cost = extracted
            .as_ref()
            .and_then(|extracted| extracted.parsed.usage)
            .and_then(|usage| usage.cost_usd);
        let wall_time_seconds = started.elapsed().as_secs_f64();
        let spend = self
            .estimate_spend(usage.clone())
            .with_wall_time(wall_time_seconds);
        let mut trace = json!({
            "kind": "cli_subagent",
            "binary": self.binary,
            "args": args,
            "stdout_path": request.output_path,
            "duration_ms": (wall_time_seconds * 1000.0).round() as u64,
            "exit_code": output.status_code,
            "pid": output.pid,
            "sandbox_backend": output.sandbox_backend,
            "sandbox_warning": output.sandbox_warning,
            "descriptor": self.descriptor.id,
            "sandbox_write_allowlist": sandbox_writes,
            "contract": {
                "active": probe.active,
                "dialect": contract.descriptor().map(|section| match section.dialect {
                    crate::registry::ContractDialect::JsonLines => "json-lines",
                    crate::registry::ContractDialect::JsonDocument => "json-document",
                }),
                "reported_cost_usd": reported_cost,
                "missing_fields": extracted.as_ref().map(|value| &value.missing_fields),
                "garbage_lines": extracted.as_ref().map(|value| value.parsed.garbage_lines),
            },
            "flight_rows": extracted
                .as_ref()
                .map(|value| flight_rows_from(&value.parsed))
                .unwrap_or_default(),
        });
        if let Some(message) = probe.caveat.as_deref() {
            add_caveat(&mut trace, "provider.contract.unavailable", message);
        }
        if degraded {
            add_caveat(
                &mut trace,
                "provider.contract.degraded",
                &format!(
                    "{} output was not its descriptor-declared structured contract; fell back to raw stdout",
                    self.descriptor.id
                ),
            );
        }
        if let Some(extracted) = &extracted {
            for field in &extracted.missing_fields {
                add_caveat(
                    &mut trace,
                    "provider.contract.capability_missing",
                    &format!(
                        "{} contract pointer for {field} did not resolve; that capability is unavailable for this turn",
                        self.descriptor.id
                    ),
                );
            }
        }

        Ok(ProviderResponse {
            provider: self.name.clone(),
            model: self.model.clone(),
            content,
            usage,
            spend,
            trace,
        })
    }

    fn render_contract_args(
        &self,
        request: &ProviderRequest,
        contract: &ProviderContract,
    ) -> Vec<String> {
        let mut args = self.render_args(request);
        let Some(section) = contract.descriptor() else {
            return args;
        };
        if contains_arg_sequence(&args, &section.stream_args) {
            return args;
        }
        let prompt_index = args
            .iter()
            .rposition(|arg| arg == &request.prompt)
            .unwrap_or(args.len());
        let insert_at = prompt_index.checked_sub(1).map_or(prompt_index, |before| {
            if prompt_value_flag_requires_next_arg(&args[before]) {
                before
            } else {
                prompt_index
            }
        });
        args.splice(insert_at..insert_at, section.stream_args.clone());
        args
    }

    fn render_args(&self, request: &ProviderRequest) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let mut inserted_model = false;
        let mut inserted_extra = false;
        for part in &self.descriptor.exec_template.args_template {
            if part == "{prompt}" {
                if args
                    .last()
                    .is_some_and(|arg| prompt_value_flag_requires_next_arg(arg))
                {
                    if let Some(prompt_flag) = args.pop() {
                        self.push_model_arg(&mut args, &mut inserted_model);
                        self.push_extra_args(&mut args, &mut inserted_extra);
                        args.push(prompt_flag);
                    }
                } else {
                    self.push_model_arg(&mut args, &mut inserted_model);
                    self.push_extra_args(&mut args, &mut inserted_extra);
                }
                args.push(request.prompt.clone());
                continue;
            }
            args.push(render_template_part(part, request));
        }
        self.push_model_arg(&mut args, &mut inserted_model);
        self.push_extra_args(&mut args, &mut inserted_extra);
        args
    }

    fn push_model_arg(&self, args: &mut Vec<String>, inserted: &mut bool) {
        if *inserted {
            return;
        }
        *inserted = true;
        let Some(flag) = self.descriptor.exec_template.model_arg.as_deref() else {
            return;
        };
        let Some(model) = self.model_arg.as_deref() else {
            return;
        };
        args.extend([flag.to_string(), model.to_string()]);
    }

    fn push_extra_args(&self, args: &mut Vec<String>, inserted: &mut bool) {
        if *inserted {
            return;
        }
        *inserted = true;
        args.extend(self.extra_args.clone());
    }

    fn sandbox_writes(&self) -> Vec<PathBuf> {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return self.descriptor.sandbox_writes.clone();
        };
        let mut paths = self
            .descriptor
            .sandbox_writes
            .iter()
            .map(|path| expand_home_path(path, &home))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
    }
}

fn prompt_value_flag_requires_next_arg(arg: &str) -> bool {
    matches!(arg, "-p" | "--prompt" | "--interactive")
}

fn contains_arg_sequence(args: &[String], expected: &[String]) -> bool {
    !expected.is_empty()
        && args
            .windows(expected.len())
            .any(|window| window == expected)
}

fn render_template_part(part: &str, request: &ProviderRequest) -> String {
    part.replace("{sandbox}", generic_sandbox_mode(request.sandbox_backend))
        .replace(
            "{cwd}",
            &request
                .cwd
                .as_ref()
                .unwrap_or(&PathBuf::new())
                .display()
                .to_string(),
        )
}

fn expand_home_path(path: &std::path::Path, home: &std::path::Path) -> PathBuf {
    if path == std::path::Path::new("~") {
        return home.to_path_buf();
    }
    if let Ok(rest) = path.strip_prefix("~") {
        return home.join(rest);
    }
    path.to_path_buf()
}

fn cli_model(model: Option<String>, descriptor: &ProviderDescriptor) -> (String, Option<String>) {
    match model {
        Some(model)
            if model.trim().is_empty() || model == descriptor.id || model == "provider default" =>
        {
            ("provider default".to_string(), None)
        }
        Some(model) => (model.clone(), Some(model)),
        None => (
            descriptor
                .default_model
                .clone()
                .unwrap_or_else(|| "provider default".to_string()),
            None,
        ),
    }
}

fn generic_sandbox_mode(outer_backend: Option<deadreckon_sandbox::SandboxBackend>) -> &'static str {
    match outer_backend {
        Some(deadreckon_sandbox::SandboxBackend::None) | None => "workspace-write",
        Some(
            deadreckon_sandbox::SandboxBackend::Auto
            | deadreckon_sandbox::SandboxBackend::SandboxExec
            | deadreckon_sandbox::SandboxBackend::Bwrap
            | deadreckon_sandbox::SandboxBackend::Docker,
        ) => "danger-full-access",
    }
}

impl Provider for GenericCliProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Generic(self.descriptor.id.clone())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn has_credential(&self) -> bool {
        which(&self.binary).is_ok() || PathBuf::from(&self.binary).exists()
    }

    fn estimate_spend(&self, usage: ProviderUsage) -> SpendEstimate {
        SpendEstimate {
            provider: self.name.clone(),
            model: self.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: 0.0,
            subscription: self.descriptor.subscription,
            wall_time_seconds: None,
        }
    }

    fn complete<'a>(&'a self, request: &'a ProviderRequest) -> ProviderFuture<'a> {
        Box::pin(async move { self.run(request).await })
    }
}

trait WithWallTime {
    fn with_wall_time(self, seconds: f64) -> Self;
}

impl WithWallTime for SpendEstimate {
    fn with_wall_time(mut self, seconds: f64) -> Self {
        self.wall_time_seconds = Some(seconds);
        self
    }
}
