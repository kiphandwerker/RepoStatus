use eframe::egui;
use rayon::prelude::*;
use git2::{BranchType, Repository};
use rfd::FileDialog;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use walkdir::WalkDir;

#[derive(Clone)]
struct RepoInfo {
    folder_path: PathBuf,
    is_git_repo: bool,
    is_github_repo: bool,
    git_status: String,
}

struct GitApp {
    root: Option<PathBuf>,
    grouped: HashMap<String, Vec<RepoInfo>>,
    scanning: bool,
    receiver: Option<Receiver<Vec<RepoInfo>>>,
}

impl Default for GitApp {
    fn default() -> Self {
        Self {
            root: None,
            grouped: HashMap::new(),
            scanning: false,
            receiver: None,
        }
    }
}

fn is_github_repo(repo: &Repository) -> bool {
    repo.remotes()
        .ok()
        .and_then(|r| {
            r.iter()
                .flatten()
                .filter_map(|n| repo.find_remote(n).ok())
                .any(|r| r.url().map(|u| u.contains("github.com")).unwrap_or(false))
                .then_some(())
        })
        .is_some()
}

fn get_git_status(repo: &Repository) -> String {
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return "NA".into(),
    };

    let branch = match head.shorthand() {
        Some(b) => b,
        None => return "NA".into(),
    };

    let local = match repo.find_branch(branch, BranchType::Local) {
        Ok(b) => b,
        Err(_) => return "NA".into(),
    };

    let upstream = match local.upstream() {
        Ok(u) => u,
        Err(_) => return "NA".into(),
    };

    let local_oid = local.get().target().unwrap();
    let upstream_oid = upstream.get().target().unwrap();

    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((ahead, behind)) => {
            if ahead == 0 && behind == 0 {
                "Current".into()
            } else if ahead > 0 && behind == 0 {
                "Ahead".into()
            } else if behind > 0 && ahead == 0 {
                "Behind".into()
            } else {
                "Diverged".into()
            }
        }
        Err(_) => "NA".into(),
    }
}


fn scan_repos(root: &Path) -> Vec<RepoInfo> {
    let mut folders = Vec::new();

    for entry in WalkDir::new(root)
        .min_depth(1)
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_dir() {
            let depth = entry.depth();

            if depth == 2 {
                folders.push(entry.path().to_path_buf());
            }
        }
    }

    folders
        .par_iter()
        .map(|path| {
            match Repository::open(path) {
                Ok(repo) => RepoInfo {
                    folder_path: path.clone(),
                    is_git_repo: true,
                    is_github_repo: is_github_repo(&repo),
                    git_status: get_git_status(&repo),
                },
                Err(_) => RepoInfo {
                    folder_path: path.clone(),
                    is_git_repo: false,
                    is_github_repo: false,
                    git_status: "Non-Git".into(),
                },
            }
        })
        .collect()
}

fn group_repos(root: &Path, repos: Vec<RepoInfo>) -> HashMap<String, Vec<RepoInfo>> {
    let mut grouped = HashMap::new();

    for repo in repos {
        let relative = repo
            .folder_path
            .strip_prefix(root)
            .unwrap_or(&repo.folder_path);

        let group = relative
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        grouped.entry(group).or_insert_with(Vec::new).push(repo);
    }

    grouped
}

impl eframe::App for GitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {

        if let Some(receiver) = &self.receiver {
            if let Ok(repos) = receiver.try_recv() {
                if let Some(root) = &self.root {
                    self.grouped = group_repos(root, repos);
                }
                self.scanning = false;
                self.receiver = None;
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {

            ui.heading("Git Repo Dashboard");

            if ui.button("Select Root Folder").clicked() && !self.scanning {
                if let Some(folder) = FileDialog::new().pick_folder() {
                    self.root = Some(folder.clone());

                    let (tx, rx) = mpsc::channel();
                    self.receiver = Some(rx);
                    self.scanning = true;

                    thread::spawn(move || {
                        let repos = scan_repos(&folder);
                        tx.send(repos).ok();
                    });
                }
            }

            if self.scanning {
                ui.label("Scanning repositories...");
            }

            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {

                for (group, repos) in &self.grouped {

                    ui.collapsing(format!("📁 {}", group), |ui| {

                    ui.set_width(ui.available_width());

                    egui::Grid::new(group)
                        .striped(true)
                        .min_col_width(120.0)
                        .show(ui, |ui| {

                            ui.label("Folder");
                            ui.label("Git");
                            ui.label("GitHub");
                            ui.label("Status");
                            ui.end_row();

                            for repo in repos {

                                let folder = repo
                                    .folder_path
                                    .file_name()
                                    .unwrap()
                                    .to_string_lossy();

                                ui.label(folder.to_string());

                                ui.label(if repo.is_git_repo { "Yes" } else { "No" });
                                
                                ui.label(if repo.is_github_repo { "Yes" } else { "-" });

                                let color = match repo.git_status.as_str() {
                                    "Ahead" => egui::Color32::LIGHT_BLUE,
                                    "Behind" => egui::Color32::RED,
                                    "Current" => egui::Color32::GREEN,
                                    "Diverged" => egui::Color32::YELLOW,
                                    _ => egui::Color32::GRAY,
                                };
                                ui.colored_label(color, &repo.git_status);

                                ui.end_row();
                            }

                        });

                    });
                }

            });

        });
    }
}

fn main() -> Result<(), eframe::Error> {

    let options = eframe::NativeOptions::default();

    eframe::run_native(
        "Git Repo Dashboard",
        options,
        Box::new(|_cc| Box::new(GitApp::default())),
    )
}
