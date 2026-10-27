//! Saving a block or an answer as a file (#72 round 2): the window's side
//! of `crate::blockexport`. "Save" writes into the chat's working folder
//! under a name that is free; "Save as…" asks with the system's dialog.
//! Either way a toast names the file, with "Open folder".

use std::path::PathBuf;

use gpui_kit::{Context, Window};

use super::Workspace;
use crate::blockexport::{self, Job};

impl Workspace {
    /// The folder a Save writes into (`blockexport::save_dir`).
    pub(super) fn export_dir(&self) -> PathBuf {
        blockexport::save_dir(
            self.model.workspace.as_ref().map(|(_, p)| p.as_path()),
            &super::cards_view::home_dir(),
        )
    }

    /// Saves `job`'s chosen format: into the folder, or where the save
    /// dialog says.
    pub(super) fn save_block(&mut self, job: Job, _window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.export_dir();
        if !job.save_as {
            let path = blockexport::unique_path(&dir, &job.file_name(job.pick));
            self.write_export(&job, job.pick, path, cx);
            return;
        }
        let picked = cx.prompt_for_new_path(&dir, Some(&job.file_name(job.pick)));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = picked.await {
                let _ = this.update(cx, |this, cx| {
                    let i = job.format_for(&path);
                    this.write_export(&job, i, path, cx);
                });
            }
        })
        .detach();
    }

    fn write_export(&mut self, job: &Job, i: usize, path: PathBuf, cx: &mut Context<Self>) {
        let Some(format) = job.formats.get(i) else {
            return;
        };
        let written = blockexport::bytes_of(format, cx)
            .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
        let now = std::time::Instant::now();
        match written {
            Ok(()) => {
                self.model.toasts.info_with(
                    format!("Saved {}", path.display()),
                    crate::toast::Retry::OpenFolder(path),
                    now,
                );
            }
            Err(e) => {
                self.model.toasts.error(
                    format!("Couldn't save {}: {e}", path.display()),
                    None,
                    now,
                );
            }
        }
        cx.notify();
    }
}
