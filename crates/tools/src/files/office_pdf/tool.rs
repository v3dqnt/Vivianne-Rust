//! `convert_to_pdf`: the model turns a Word, Excel or PowerPoint file in the project into a PDF beside it (or where it says),
//! which it can then hand over with `present` or read. Offered only on a machine that has a LibreOffice to do the work.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{Code, Config, Error, OfficeToPdf, Priority};
use crate::simple::Workspace;
use crate::{Tool, ToolResult};

/// The tool's name.
pub const NAME: &str = "convert_to_pdf";

/// One converter for the whole process, so every conversation shares its queue limits and its kept PDFs.
fn system() -> &'static Result<OfficeToPdf, Error> {
    static SERVICE: OnceLock<Result<OfficeToPdf, Error>> = OnceLock::new();
    SERVICE.get_or_init(|| OfficeToPdf::from_system(Config::default()))
}

/// Whether this machine can convert: an installed LibreOffice was found.
#[must_use]
pub fn available() -> bool {
    super::find_soffice().is_some()
}

/// Converts an Office file in the working folder to a PDF.
#[derive(Debug)]
pub struct ConvertToPdf {
    workspace: Workspace,
    service: Option<Arc<OfficeToPdf>>,
}

impl ConvertToPdf {
    /// Over the process-wide converter, working in `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            workspace: Workspace::new(root),
            service: None,
        }
    }

    /// Over a converter of the caller's own (a made-up engine, in tests).
    #[must_use]
    pub fn with_service(root: &Path, service: Arc<OfficeToPdf>) -> Self {
        Self {
            workspace: Workspace::new(root),
            service: Some(service),
        }
    }

    fn converter(&self) -> Result<&OfficeToPdf, Error> {
        match &self.service {
            Some(service) => Ok(service),
            None => system().as_ref().map_err(Clone::clone),
        }
    }
}

/// What to tell the model about a failed conversion: the class, never the engine's own words.
fn reason(error: &Error) -> String {
    match error.code {
        Code::UnsupportedFormat => {
            "the file must end in .doc, .docx, .xls, .xlsx, .ppt or .pptx".to_owned()
        }
        Code::InputTooLarge => "the document is too large to convert".to_owned(),
        Code::OutputTooLarge => "the converted PDF is too large".to_owned(),
        Code::InvalidDocument => {
            "the document could not be opened; it may be damaged or protected".to_owned()
        }
        Code::Timeout => "the conversion took too long".to_owned(),
        Code::Unavailable => "LibreOffice is not installed on this machine".to_owned(),
        Code::Busy => "the converter is busy; try again shortly".to_owned(),
        Code::SourceChanged => {
            "the file changed while it was being converted; try again".to_owned()
        }
        Code::Cancelled => "the conversion was cancelled".to_owned(),
        Code::InvalidOutput | Code::Failed => error.message.clone(),
    }
}

#[async_trait]
impl Tool for ConvertToPdf {
    fn name(&self) -> &str {
        NAME
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": NAME,
            "description": "Convert a Word, Excel or PowerPoint file (.doc, .docx, .xls, .xlsx, .ppt, .pptx) in the project to a PDF. The PDF is written next to the file with the same name unless you give an output path; an existing file is never replaced unless overwrite is true. Use present afterwards to hand the PDF to the user. Needs LibreOffice on this machine.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path of the Office file" },
                    "output": { "type": "string", "description": "Relative path of the PDF to write (default: the same path with a .pdf extension)" },
                    "overwrite": { "type": "boolean", "description": "Replace the output if it already exists (default false)" }
                },
                "required": ["path"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(path) = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|p| !p.trim().is_empty())
        else {
            return ToolResult::error("convert_to_pdf requires a path");
        };
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let input = match self.workspace.resolve(path) {
            Ok(input) => input,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let output = match args
            .get("output")
            .and_then(Value::as_str)
            .filter(|o| !o.trim().is_empty())
        {
            Some(output) => match self.workspace.resolve(output) {
                Ok(output) => output,
                Err(error) => return ToolResult::error(error.to_string()),
            },
            None => input.with_extension("pdf"),
        };
        let shown = self.workspace.display(&output);
        if !shown.to_ascii_lowercase().ends_with(".pdf") {
            return ToolResult::error("the output path must end in .pdf");
        }
        // Checked before converting so a refused overwrite costs nothing.
        if output.exists() && !overwrite {
            return ToolResult::error(format!(
                "{shown} already exists; choose another output or set overwrite to true"
            ));
        }
        let service = match self.converter() {
            Ok(service) => service,
            Err(error) => return ToolResult::error(format!("cannot convert: {}", reason(&error))),
        };
        let rendered = match service
            .render_file(&input, Priority::Foreground, &CancellationToken::new())
            .await
        {
            Ok(rendered) => rendered,
            // The engine's own words stay in `error.detail`; the model gets the class.
            Err(error) => {
                return ToolResult::error(format!("cannot convert {path}: {}", reason(&error)));
            }
        };
        if let Some(folder) = output.parent()
            && let Err(error) = tokio::fs::create_dir_all(folder).await
        {
            return ToolResult::error(format!("cannot write {shown}: {error}"));
        }
        if let Err(error) = tokio::fs::write(&output, &rendered.pdf).await {
            return ToolResult::error(format!("cannot write {shown}: {error}"));
        }
        let mut content = format!(
            "Converted {path} to {shown} ({} KB).",
            rendered.pdf.len().div_ceil(1024)
        );
        if !rendered.missing_fonts.is_empty() {
            content = format!(
                "{content} Fonts that were not available: {}.",
                rendered.missing_fonts.join(", ")
            );
        }
        ToolResult {
            content,
            is_error: false,
            details: Some(json!({ "output": shown })),
        }
    }
}
