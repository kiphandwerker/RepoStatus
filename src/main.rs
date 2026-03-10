// #![windows_subsystem = "windows"]
use eframe::egui;
use git2::{FetchOptions, RemoteCallbacks, Repository};
use rayon::prelude::*;
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
    println!("--------------------------------");
    println!("Checking repo: {}", repo.path().display());

    // HEAD
    let head = match repo.head() {
        Ok(h) if h.is_branch() => {
            println!("HEAD reference OK");
            h
        }
        Ok(_) => {
            println!("HEAD is not a branch (detached)");
            return "NA".into();
        }
        Err(e) => {
            println!("HEAD error: {}", e);
            return "NA".into();
        }
    };

    // branch name
    let branch_name = match head.shorthand() {
        Some(name) => {
            println!("Current branch: {}", name);
            name
        }
        None => {
            println!("Could not determine branch name");
            return "NA".into();
        }
    };

    // local branch
    let branch = match repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(b) => {
            println!("Local branch found");
            b
        }
        Err(e) => {
            println!("Error finding local branch: {}", e);
            return "NA".into();
        }
    };

    // upstream branch
    let upstream = match branch.upstream() {
        Ok(u) => {
            println!("Upstream branch found");
            u
        }
        Err(e) => {
            println!("No upstream branch: {}", e);
            return "NA".into();
        }
    };

    // local commit
    let local_oid = match branch.get().target() {
        Some(id) => {
            println!("Local commit: {}", id);
            id
        }
        None => {
            println!("Local branch has no commit");
            return "NA".into();
        }
    };

    // remote commit
    let remote_oid = match upstream.get().target() {
        Some(id) => {
            println!("Remote commit: {}", id);
            id
        }
        None => {
            println!("Remote branch has no commit");
            return "NA".into();
        }
    };

    // ahead/behind calculation
    let (ahead, behind) = match repo.graph_ahead_behind(local_oid, remote_oid) {
        Ok(result) => {
            println!("Ahead: {}, Behind: {}", result.0, result.1);
            result
        }
        Err(e) => {
            println!("graph_ahead_behind error: {}", e);
            return "NA".into();
        }
    };

    let status = match (ahead, behind) {
        (0, 0) => "Current",
        (a, 0) if a > 0 => "Ahead",
        (0, b) if b > 0 => "Behind",
        _ => "Diverged",
    };

    println!("Final Status: {}", status);
    println!("--------------------------------");

    status.into()
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
        .map(|path| match Repository::open(path) {
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

            ui.label("Author: Kip Handwerker");
            ui.separator();

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
                                    let folder =
                                        repo.folder_path.file_name().unwrap().to_string_lossy();

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
