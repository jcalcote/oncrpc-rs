use clap::{Args, Parser, Subcommand};
use onc_rpcgen::{
    GenerateOptions, GeneratedModuleOutput, GeneratorError, LoadOptions,
    emit_rust_stubs_for_module_with_options, emit_rust_types_for_module,
    generate_from_x_file_with_options, load_module_set_from_x_file_with_options,
    parse_x_file_with_options,
};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "oncrpcgen")]
#[command(about = "Generate Rust XDR types and ONC RPC stubs from .x files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Parse(ParseCommand),
    Emit(EmitCommand),
    Generate(GenerateCommand),
}

#[derive(Debug, Args)]
struct CommonInput {
    #[arg(long = "include-dir")]
    include_dirs: Vec<PathBuf>,
    #[arg(long = "emit-ast")]
    emit_ast: bool,
    #[arg(long = "module")]
    module_name: Option<String>,
    x_file: PathBuf,
}

#[derive(Debug, Args)]
struct ParseCommand {
    #[command(flatten)]
    input: CommonInput,
}

#[derive(Debug, Args)]
struct EmitCommand {
    #[command(subcommand)]
    kind: EmitKindCommand,
}

#[derive(Debug, Subcommand)]
enum EmitKindCommand {
    Types(FileEmitCommand),
    Stubs(FileEmitCommand),
}

#[derive(Debug, Args)]
struct FileEmitCommand {
    #[command(flatten)]
    input: CommonInput,
    #[arg(long = "out-dir")]
    out_dir: PathBuf,
    #[arg(long = "no-client")]
    no_client: bool,
    #[arg(long = "no-server")]
    no_server: bool,
    #[arg(long = "verbose")]
    verbose: bool,
}

#[derive(Debug, Args)]
struct GenerateCommand {
    #[command(flatten)]
    input: CommonInput,
    #[arg(long = "out-dir")]
    out_dir: PathBuf,
    #[arg(long = "no-types")]
    no_types: bool,
    #[arg(long = "no-stubs")]
    no_stubs: bool,
    #[arg(long = "no-client")]
    no_client: bool,
    #[arg(long = "no-server")]
    no_server: bool,
    #[arg(long = "verbose")]
    verbose: bool,
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), GeneratorError> {
    match cli.command {
        Command::Parse(command) => {
            let schema = parse_x_file_with_options(
                &command.input.x_file,
                &to_load_options(&command.input.include_dirs),
            )?;
            if command.input.emit_ast {
                print!("{}", schema.render_snapshot());
            }
            Ok(())
        }
        Command::Emit(command) => match command.kind {
            EmitKindCommand::Types(command) => {
                let load = to_load_options(&command.input.include_dirs);
                let schema = parse_x_file_with_options(&command.input.x_file, &load)?;
                maybe_print_ast(&schema, command.input.emit_ast);
                let loaded =
                    load_module_set_from_x_file_with_options(&command.input.x_file, &load)?;
                let mut outputs = Vec::new();
                for module in &loaded.modules {
                    outputs.push(GeneratedModuleOutput {
                        module_name: module.module_name.clone(),
                        types: {
                            let output = emit_rust_types_for_module(module, &loaded)?;
                            if output.trim().is_empty() {
                                None
                            } else {
                                Some(output)
                            }
                        },
                        stubs: None,
                    });
                }
                write_generated_outputs(&command.out_dir, &outputs, command.verbose)
            }
            EmitKindCommand::Stubs(command) => {
                let load = to_load_options(&command.input.include_dirs);
                let schema = parse_x_file_with_options(&command.input.x_file, &load)?;
                maybe_print_ast(&schema, command.input.emit_ast);
                let loaded =
                    load_module_set_from_x_file_with_options(&command.input.x_file, &load)?;
                let mut outputs = Vec::new();
                for module in &loaded.modules {
                    if module.module_name != loaded.root_module {
                        continue;
                    }
                    outputs.push(GeneratedModuleOutput {
                        module_name: module.module_name.clone(),
                        types: None,
                        stubs: {
                            let output = emit_rust_stubs_for_module_with_options(
                                module,
                                &loaded,
                                !command.no_client,
                                !command.no_server,
                            )?;
                            if output.trim().is_empty() {
                                None
                            } else {
                                Some(output)
                            }
                        },
                    });
                }
                write_generated_outputs(&command.out_dir, &outputs, command.verbose)
            }
        },
        Command::Generate(command) => {
            let load = to_load_options(&command.input.include_dirs);
            let schema = parse_x_file_with_options(&command.input.x_file, &load)?;
            maybe_print_ast(&schema, command.input.emit_ast);
            let outputs = generate_from_x_file_with_options(
                &command.input.x_file,
                &load,
                &GenerateOptions {
                    module_name: command.input.module_name.clone(),
                    emit_types: !command.no_types,
                    emit_stubs: !command.no_stubs,
                    emit_client: !command.no_client,
                    emit_server: !command.no_server,
                },
            )?;
            write_generated_outputs(&command.out_dir, &outputs.modules, command.verbose)
        }
    }
}

fn to_load_options(include_dirs: &[PathBuf]) -> LoadOptions {
    LoadOptions {
        include_dirs: include_dirs.to_vec(),
    }
}

fn maybe_print_ast(schema: &onc_rpcgen::Schema, emit_ast: bool) {
    if emit_ast {
        print!("{}", schema.render_snapshot());
    }
}

fn write_generated_outputs(
    out_dir: &Path,
    outputs: &[GeneratedModuleOutput],
    verbose: bool,
) -> Result<(), GeneratorError> {
    fs::create_dir_all(out_dir).map_err(|error| GeneratorError::Io {
        path: out_dir.display().to_string(),
        message: error.to_string(),
    })?;

    for output in outputs {
        if let Some(types) = &output.types {
            write_output(
                out_dir,
                &format!("{}.types.rs", output.module_name),
                types,
                verbose,
            )?;
        }
        if let Some(stubs) = &output.stubs {
            write_output(
                out_dir,
                &format!("{}.stubs.rs", output.module_name),
                stubs,
                verbose,
            )?;
        }
    }

    Ok(())
}

fn write_output(
    out_dir: &Path,
    filename: &str,
    output: &str,
    verbose: bool,
) -> Result<(), GeneratorError> {
    fs::create_dir_all(out_dir).map_err(|error| GeneratorError::Io {
        path: out_dir.display().to_string(),
        message: error.to_string(),
    })?;
    let path = out_dir.join(filename);
    fs::write(&path, output).map_err(|error| GeneratorError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    if verbose {
        println!("wrote {}", path.display());
    }
    Ok(())
}
