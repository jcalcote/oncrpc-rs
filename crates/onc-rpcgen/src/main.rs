use clap::{Args, Parser, Subcommand};
use onc_rpcgen::{
    GenerateOptions, GeneratorError, LoadOptions, emit_rust_stubs_from_x_file_with_options,
    emit_rust_types_from_x_file_with_options, generate_from_x_file_with_options,
    module_name_for_path, parse_x_file_with_options,
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
                let output =
                    emit_rust_types_from_x_file_with_options(&command.input.x_file, &load)?;
                let module = module_name_for_path(
                    &command.input.x_file,
                    command.input.module_name.as_deref(),
                );
                write_output(&command.out_dir, &format!("{module}.types.rs"), &output)
            }
            EmitKindCommand::Stubs(command) => {
                let load = to_load_options(&command.input.include_dirs);
                let schema = parse_x_file_with_options(&command.input.x_file, &load)?;
                maybe_print_ast(&schema, command.input.emit_ast);
                let output =
                    emit_rust_stubs_from_x_file_with_options(&command.input.x_file, &load)?;
                let module = module_name_for_path(
                    &command.input.x_file,
                    command.input.module_name.as_deref(),
                );
                write_output(&command.out_dir, &format!("{module}.stubs.rs"), &output)
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
                },
            )?;
            fs::create_dir_all(&command.out_dir).map_err(|error| GeneratorError::Io {
                path: command.out_dir.display().to_string(),
                message: error.to_string(),
            })?;
            if let Some(types) = outputs.types {
                write_output(
                    &command.out_dir,
                    &format!("{}.types.rs", outputs.module_name),
                    &types,
                )?;
            }
            if let Some(stubs) = outputs.stubs {
                write_output(
                    &command.out_dir,
                    &format!("{}.stubs.rs", outputs.module_name),
                    &stubs,
                )?;
            }
            Ok(())
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

fn write_output(out_dir: &Path, filename: &str, output: &str) -> Result<(), GeneratorError> {
    fs::create_dir_all(out_dir).map_err(|error| GeneratorError::Io {
        path: out_dir.display().to_string(),
        message: error.to_string(),
    })?;
    let path = out_dir.join(filename);
    fs::write(&path, output).map_err(|error| GeneratorError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    Ok(())
}
