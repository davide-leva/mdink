mod assets;
mod layout;
mod markdown;
mod theme;

use std::{env, fs, path::PathBuf, process};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HELP: &str = "Usage: mdink <document.md> [options]

Convert Markdown to A4 PDF. No browser or Node.js required.

  -o, --output <file.pdf>  Output path (default: next to the input file)
      --title <title>     PDF metadata title
      --landscape        Landscape orientation
      --theme <name>     Color theme (default: ocean)
      --color <#RRGGBB>  Custom primary color
      --list-themes      List the six available themes
  -h, --help             Show this help message
  -v, --version          Show the version";

fn run() -> Result<()> {
    let mut input = None;
    let mut output = None;
    let mut title = None;
    let mut landscape = false;
    let mut theme_name = String::from("ocean");
    let mut color = None;
    let mut positional = false;
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if !positional {
            match arg.to_str() {
                Some("--") => {
                    positional = true;
                    continue;
                }
                Some("-h" | "--help") => {
                    println!("{HELP}");
                    return Ok(());
                }
                Some("-v" | "--version") => {
                    println!("{}", env!("CARGO_PKG_VERSION"));
                    return Ok(());
                }
                Some("--landscape") => {
                    landscape = true;
                    continue;
                }
                Some("--list-themes") => {
                    for (name, color) in theme::THEMES {
                        println!("{name:12} {color}");
                    }
                    return Ok(());
                }
                Some("--theme") => {
                    theme_name = args
                        .next()
                        .ok_or("Missing theme name after --theme")?
                        .into_string()
                        .map_err(|_| "Theme name must be valid UTF-8")?;
                    continue;
                }
                Some("--color") => {
                    color = Some(
                        args.next()
                            .ok_or("Missing color after --color")?
                            .into_string()
                            .map_err(|_| "Invalid color")?,
                    );
                    continue;
                }
                Some("-o" | "--output") => {
                    output = Some(PathBuf::from(
                        args.next().ok_or("Missing path after --output")?,
                    ));
                    continue;
                }
                Some("--title") => {
                    title = Some(
                        args.next()
                            .ok_or("Missing title after --title")?
                            .into_string()
                            .map_err(|_| "Title must be valid UTF-8")?,
                    );
                    continue;
                }
                Some(s) if s.starts_with('-') => {
                    return Err(format!("Unknown option: {s}").into());
                }
                _ => {}
            }
        }
        if input.replace(PathBuf::from(arg)).is_some() {
            return Err("Expected exactly one Markdown file".into());
        }
    }
    let input = input.ok_or("Specify a Markdown file. Use --help for usage instructions")?;
    let input = fs::canonicalize(input)?;
    let output = std::path::absolute(output.unwrap_or_else(|| input.with_extension("pdf")))?;
    if output
        .extension()
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("pdf"))
    {
        return Err("The output file must have a .pdf extension".into());
    }
    if output == input || fs::canonicalize(&output).is_ok_and(|p| p == input) {
        return Err("The output file must be different from the input file".into());
    }
    let title = title.unwrap_or_else(|| {
        input
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let source = fs::read_to_string(&input)?;
    let blocks = markdown::parse(&source);
    let theme = theme::Theme::new(&theme_name, color.as_deref())?;
    let mut renderer = layout::Layout::new(input.parent().unwrap(), landscape, theme)?;
    renderer.render(&blocks)?;
    let pdf = renderer.finish(&title)?;
    let parent = output.parent().ok_or("Invalid output path")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temporary.write_all(&pdf)?;
    temporary.persist(&output)?;
    println!("PDF created: {}", output.display());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        process::exit(1);
    }
}
