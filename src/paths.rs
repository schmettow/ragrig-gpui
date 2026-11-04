//! Path display and file dialogs with a starting directory.
//!
//! GPUI's [`App::prompt_for_paths`] has no option for the folder a dialog
//! opens in. On Linux the XDG portal does, so [`prompt_for_paths_at`] asks the
//! portal directly; other platforms use GPUI's prompt, whose default is the
//! process working directory — which Scico points at the workspace.
//!
//! The helpers here also keep long home-directory paths readable: settings
//! show `~/paper` while the application continues to store absolute paths.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use gpui_kit::{App, PathPromptOptions};

/// Render `path` below the home directory as `~/…`.
///
/// Paths outside the home directory are returned unchanged. The function is
/// display-only; [`expand_home`] turns the result back into a path.
pub fn home_relative(path: &Path) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Expand a leading `~` to the home directory; other values are kept as given.
pub fn expand_home(value: &str) -> PathBuf {
    let value = value.trim();
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

/// The folder a **Browse…** dialog opens in.
///
/// The current path when it names a directory, otherwise its existing parent,
/// otherwise the process working directory (the workspace in Scico), then the
/// home directory.
pub(crate) fn start_directory(current: &str) -> Option<PathBuf> {
    let path = expand_home(current);
    if path.as_os_str().is_empty() {
        return std::env::current_dir().ok().or_else(dirs::home_dir);
    }
    if path.is_dir() {
        return Some(path);
    }
    path.parent()
        .filter(|parent| parent.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .or_else(dirs::home_dir)
}

/// The future returned by [`prompt_for_paths_at`].
pub type PathPrompt = Pin<Box<dyn Future<Output = Result<Option<Vec<PathBuf>>, String>> + 'static>>;

/// Prompt for files or directories, preferring `start` as the initial folder.
///
/// The future is `'static`, so a GPUI host can run it on its executor.
/// Cancelling the dialog yields `Ok(None)`; a dialog that cannot be opened
/// yields a readable error.
pub fn prompt_for_paths_at(
    app: &App,
    start: Option<PathBuf>,
    options: PathPromptOptions,
) -> PathPrompt {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        Box::pin(portal::prompt(start, options))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // The platform prompt has no starting-folder argument; it begins at
        // the process working directory, which the host points at the
        // workspace.
        let _ = start;
        let receiver = app.prompt_for_paths(options);
        Box::pin(async move {
            match receiver.await {
                Ok(Ok(paths)) => Ok(paths),
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err("the file dialog stopped without a result".to_owned()),
            }
        })
    }
}

#[cfg(target_os = "linux")]
mod portal {
    use super::*;
    use ashpd::desktop::file_chooser::OpenFileRequest;

    pub(super) async fn prompt(
        start: Option<PathBuf>,
        options: PathPromptOptions,
    ) -> Result<Option<Vec<PathBuf>>, String> {
        let title = if options.directories {
            "Open Folder"
        } else {
            "Open File"
        };
        let mut request = OpenFileRequest::default()
            .modal(true)
            .title(title)
            .multiple(options.multiple)
            .directory(options.directories);
        if let Some(start) = start {
            request = request
                .current_folder(start)
                .map_err(|error| format!("invalid start folder: {error}"))?;
        }
        let selected = request
            .send()
            .await
            .map_err(|error| format!("cannot open the file dialog: {error}"))?
            .response()
            .map_err(|error| format!("the file dialog failed: {error}"))?;
        let paths: Vec<PathBuf> = selected
            .uris()
            .iter()
            .filter_map(|uri| url::Url::parse(uri.as_str()).ok())
            .filter_map(|url| url.to_file_path().ok())
            .collect();
        Ok((!paths.is_empty()).then_some(paths))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_paths_are_shown_with_a_tilde() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert_eq!(home_relative(&home), "~");
        assert_eq!(
            home_relative(&home.join("paper")),
            format!("~{}{}", std::path::MAIN_SEPARATOR, "paper")
        );
        assert_eq!(expand_home("~/paper"), home.join("paper"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(
            expand_home("/outside/paper"),
            PathBuf::from("/outside/paper")
        );
    }

    #[test]
    fn a_browse_dialog_starts_at_the_current_path() {
        let dir = std::env::temp_dir().join(format!(
            "ragrig-gpui-paths-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(
            start_directory(&dir.display().to_string()),
            Some(dir.clone())
        );

        let file = dir.join("paper.md");
        std::fs::write(&file, "# Paper").expect("paper");
        assert_eq!(
            start_directory(&file.display().to_string()),
            Some(dir.clone())
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
