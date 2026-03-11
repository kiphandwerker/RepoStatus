// #![windows_subsystem = "windows"]

use eframe::egui;
use git2::{FetchOptions, RemoteCallbacks, Repository};
use rayon::prelude::*;
use rfd::FileDialog;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use walkdir::WalkDir;

// ── Data ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct RepoInfo {
    folder_path: PathBuf,
    is_git_repo: bool,
    is_github_repo: bool,
    /// "Current" | "Ahead" | "Behind" | "Diverged" | "No Upstream" | "NA" | "Non-Git"
    git_status: String,
}

#[derive(Default)]
struct StatusCounts {
    current: usize,
    ahead: usize,
    behind: usize,
    diverged: usize,
    no_upstream: usize,
    na: usize,
    non_git: usize,
}

impl StatusCounts {
    fn from_repos(repos: &HashMap<String, Vec<RepoInfo>>) -> Self {
        let mut c = Self::default();
        for repo in repos.values().flatten() {
            match repo.git_status.as_str() {
                "Current" => c.current += 1,
                "Ahead" => c.ahead += 1,
                "Behind" => c.behind += 1,
                "Diverged" => c.diverged += 1,
                "No Upstream" => c.no_upstream += 1,
                "Non-Git" => c.non_git += 1,
                _ => c.na += 1,
            }
        }
        c
    }

    fn total_git(&self) -> usize {
        self.current + self.ahead + self.behind + self.diverged + self.no_upstream + self.na
    }
}

// ── Git helpers ───────────────────────────────────────────────────────────────

fn is_github_repo(repo: &Repository) -> bool {
    let Ok(remotes) = repo.remotes() else {
        return false;
    };
    remotes
        .iter()
        .flatten()
        .filter_map(|name| repo.find_remote(name).ok())
        .any(|r| r.url().map_or(false, |u| u.contains("github.com")))
}

fn get_git_status(repo: &Repository) -> String {
    let head = match repo.head() {
        Ok(h) if h.is_branch() => h,
        _ => return "NA".into(),
    };

    let branch_name = match head.shorthand() {
        Some(n) => n,
        None => return "NA".into(),
    };

    let branch = match repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return "NA".into(),
    };

    let upstream = match branch.upstream() {
        Ok(u) => u,
        Err(_) => return "No Upstream".into(), // clearer than "NA"
    };

    let local_oid = match branch.get().target() {
        Some(id) => id,
        None => return "NA".into(),
    };

    let remote_oid = match upstream.get().target() {
        Some(id) => id,
        None => return "NA".into(),
    };

    match repo.graph_ahead_behind(local_oid, remote_oid) {
        Ok((0, 0)) => "Current".into(),
        Ok((a, 0)) if a > 0 => "Ahead".into(),
        Ok((0, b)) if b > 0 => "Behind".into(),
        Ok(_) => "Diverged".into(),
        Err(_) => "NA".into(),
    }
}

/// Fetch all remotes for a repo using unauthenticated (HTTPS public) or
/// SSH-agent credentials.  Errors are silently swallowed — the caller only
/// cares whether it succeeded.
fn fetch_all_remotes(repo: &Repository) {
    let Ok(remote_names) = repo.remotes() else {
        return;
    };

    for name in remote_names.iter().flatten() {
        let Ok(mut remote) = repo.find_remote(name) else {
            continue;
        };

        let mut callbacks = RemoteCallbacks::new();
        // Try SSH agent first, fall back to no credentials (public HTTPS repos).
        callbacks.credentials(|_url, username, _allowed| {
            git2::Cred::ssh_key_from_agent(username.unwrap_or("git"))
        });

        let mut opts = FetchOptions::new();
        opts.remote_callbacks(callbacks);

        // Fetch all branches; ignore individual remote errors.
        let _ = remote.fetch(&[] as &[&str], Some(&mut opts), None);
    }
}

// ── Scanning ──────────────────────────────────────────────────────────────────

fn scan_repos(root: &Path) -> Vec<RepoInfo> {
    // Collect candidate directories at depth 1 AND 2.
    let folders: Vec<PathBuf> = WalkDir::new(root)
        .min_depth(1)
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir() && e.depth() >= 1)
        .map(|e| e.path().to_path_buf())
        .collect();

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

fn group_repos(root: &Path, mut repos: Vec<RepoInfo>) -> HashMap<String, Vec<RepoInfo>> {
    // Sort repos by their full path so each group is alphabetically ordered.
    repos.sort_by(|a, b| a.folder_path.cmp(&b.folder_path));

    let mut grouped: HashMap<String, Vec<RepoInfo>> = HashMap::new();

    for repo in repos {
        let relative = repo
            .folder_path
            .strip_prefix(root)
            .unwrap_or(&repo.folder_path);

        // Depth-1 repos: use the repo folder itself as the group name with a
        // special sentinel so the UI can render it differently.
        let group = relative
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        grouped.entry(group).or_default().push(repo);
    }

    grouped
}

// ── Background worker messages ────────────────────────────────────────────────

enum WorkerMsg {
    /// Initial scan finished.
    ScanDone(Vec<RepoInfo>),
    /// A fetch-all pass finished; contains refreshed repo list.
    FetchDone(Vec<RepoInfo>),
}

// ── App state ─────────────────────────────────────────────────────────────────

struct GitApp {
    root: Option<PathBuf>,
    grouped: HashMap<String, Vec<RepoInfo>>,
    counts: StatusCounts,
    scanning: bool,
    fetching: bool,
    status_msg: String,
    receiver: Option<Receiver<WorkerMsg>>,
    /// Which groups are currently expanded (open collapsibles).
    open_groups: HashMap<String, bool>,
    /// Whether to hide Non-Git folders.
    hide_non_git: bool,
}

impl Default for GitApp {
    fn default() -> Self {
        Self {
            root: None,
            grouped: HashMap::new(),
            counts: StatusCounts::default(),
            scanning: false,
            fetching: false,
            status_msg: String::new(),
            receiver: None,
            open_groups: HashMap::new(),
            hide_non_git: true,
        }
    }
}

impl GitApp {
    fn start_scan(&mut self, folder: PathBuf, ctx: egui::Context) {
        self.root = Some(folder.clone());
        self.scanning = true;
        self.status_msg = "Scanning…".into();

        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        self.receiver = Some(rx);

        thread::spawn(move || {
            let repos = scan_repos(&folder);
            tx.send(WorkerMsg::ScanDone(repos)).ok();
            ctx.request_repaint();
        });
    }

    fn start_fetch(&mut self, ctx: egui::Context) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.fetching = true;
        self.status_msg = "Fetching remotes…".into();

        let repos_snapshot: Vec<RepoInfo> = self.grouped.values().flatten().cloned().collect();
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        self.receiver = Some(rx);

        thread::spawn(move || {
            // Fetch every git repo in parallel.
            repos_snapshot.par_iter().for_each(|info| {
                if info.is_git_repo {
                    if let Ok(repo) = Repository::open(&info.folder_path) {
                        fetch_all_remotes(&repo);
                    }
                }
            });

            // Re-scan to pick up new upstream states.
            let updated = scan_repos(&root);
            tx.send(WorkerMsg::FetchDone(updated)).ok();
            ctx.request_repaint();
        });
    }

    fn handle_worker_msg(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::ScanDone(repos) | WorkerMsg::FetchDone(repos) => {
                if let Some(root) = &self.root {
                    self.grouped = group_repos(root, repos);
                    self.counts = StatusCounts::from_repos(&self.grouped);
                }
                self.scanning = false;
                self.fetching = false;
                self.receiver = None;
                self.status_msg = format!(
                    "✅ {} git repos — {} current, {} ahead, {} behind, {} diverged",
                    self.counts.total_git(),
                    self.counts.current,
                    self.counts.ahead,
                    self.counts.behind,
                    self.counts.diverged,
                );
            }
        }
    }
}

// ── UI ────────────────────────────────────────────────────────────────────────

fn status_color(status: &str) -> egui::Color32 {
    match status {
        "Current" => egui::Color32::from_rgb(80, 200, 120),
        "Ahead" => egui::Color32::from_rgb(100, 180, 255),
        "Behind" => egui::Color32::from_rgb(255, 80, 80),
        "Diverged" => egui::Color32::from_rgb(255, 200, 50),
        "No Upstream" => egui::Color32::from_rgb(180, 130, 255),
        "Non-Git" => egui::Color32::from_rgb(120, 120, 120),
        _ => egui::Color32::DARK_GRAY,
    }
}

impl eframe::App for GitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll background worker.
        if let Some(rx) = &self.receiver {
            if let Ok(msg) = rx.try_recv() {
                self.handle_worker_msg(msg);
            } else if self.scanning || self.fetching {
                // Keep repainting while work is in progress.
                ctx.request_repaint();
            }
        }

        let busy = self.scanning || self.fetching;

        egui::CentralPanel::default().show(ctx, |ui| {
            // ── Header ──────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.heading("🗂  Git Repo Dashboard");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small("by Kip Handwerker");
                });
            });
            ui.separator();

            // ── Toolbar ─────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("📂 Select Folder"))
                    .clicked()
                {
                    if let Some(folder) = FileDialog::new().pick_folder() {
                        self.open_groups.clear();
                        self.start_scan(folder, ctx.clone());
                    }
                }

                if self.root.is_some() {
                    if ui
                        .add_enabled(!busy, egui::Button::new("🔄 Refresh"))
                        .on_hover_text("Re-scan without fetching")
                        .clicked()
                    {
                        if let Some(root) = self.root.clone() {
                            self.start_scan(root, ctx.clone());
                        }
                    }

                    if ui
                        .add_enabled(!busy, egui::Button::new("⬇  Fetch All"))
                        .on_hover_text("git fetch for every repo, then refresh status")
                        .clicked()
                    {
                        self.start_fetch(ctx.clone());
                    }
                }

                ui.checkbox(&mut self.hide_non_git, "Hide non-Git folders");
            });

            // Root path label.
            if let Some(root) = &self.root {
                ui.horizontal(|ui| {
                    ui.label("Root:");
                    ui.monospace(root.display().to_string());
                });
            }

            // Status / progress message.
            if !self.status_msg.is_empty() {
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&self.status_msg.clone());
                    });
                } else {
                    ui.label(&self.status_msg.clone());
                }
            }

            ui.separator();

            // ── Summary pills ────────────────────────────────────────────────
            if self.counts.total_git() > 0 {
                ui.horizontal(|ui| {
                    let pills = [
                        ("Current", self.counts.current, egui::Color32::from_rgb(80, 200, 120)),
                        ("Ahead", self.counts.ahead, egui::Color32::from_rgb(100, 180, 255)),
                        ("Behind", self.counts.behind, egui::Color32::from_rgb(255, 80, 80)),
                        ("Diverged", self.counts.diverged, egui::Color32::from_rgb(255, 200, 50)),
                        ("No Upstream", self.counts.no_upstream, egui::Color32::from_rgb(180, 130, 255)),
                    ];
                    for (label, count, color) in &pills {
                        if *count > 0 {
                            ui.colored_label(*color, format!("{label}: {count}"));
                            ui.separator();
                        }
                    }
                });
                ui.add_space(4.0);
            }

            // ── Repo table ───────────────────────────────────────────────────
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Sort group names for stable ordering.
                let mut group_names: Vec<String> = self.grouped.keys().cloned().collect();
                group_names.sort();

                for group in &group_names {
                    let repos = &self.grouped[group];

                    // Optionally filter groups that contain only non-git folders.
                    if self.hide_non_git && repos.iter().all(|r| !r.is_git_repo) {
                        continue;
                    }

                    let git_count = repos.iter().filter(|r| r.is_git_repo).count();
                    let header = format!("📁  {}  ({} git)", group, git_count);

                    // Track open/closed state per group; default to open.
                    let open = self.open_groups.entry(group.clone()).or_insert(true);

                    let id = ui.make_persistent_id(group);
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ctx, id, *open,
                    )
                    .show_header(ui, |ui| {
                        ui.strong(&header);
                    })
                    .body(|ui| {
                        egui::Grid::new(group)
                            .striped(true)
                            .min_col_width(100.0)
                            .num_columns(4)
                            .show(ui, |ui| {
                                // Column headers.
                                ui.strong("Folder");
                                ui.strong("Git");
                                ui.strong("GitHub");
                                ui.strong("Status");
                                ui.end_row();

                                for repo in repos {
                                    // Skip non-git rows when filter is active.
                                    if self.hide_non_git && !repo.is_git_repo {
                                        continue;
                                    }

                                    let name = repo
                                        .folder_path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_default();

                                    // Folder name — clicking opens it in the OS file manager.
                                    if ui
                                        .link(&name)
                                        .on_hover_text(repo.folder_path.display().to_string())
                                        .clicked()
                                    {
                                        #[cfg(target_os = "windows")]
                                        let _ = std::process::Command::new("explorer")
                                            .arg(&repo.folder_path)
                                            .spawn();
                                        #[cfg(target_os = "macos")]
                                        let _ = std::process::Command::new("open")
                                            .arg(&repo.folder_path)
                                            .spawn();
                                        #[cfg(target_os = "linux")]
                                        let _ = std::process::Command::new("xdg-open")
                                            .arg(&repo.folder_path)
                                            .spawn();
                                    }

                                    ui.label(if repo.is_git_repo { "✔" } else { "–" });
                                    ui.label(if repo.is_github_repo { "✔" } else { "–" });

                                    let color = status_color(&repo.git_status);
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

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([700.0, 520.0])
            .with_title("Git Repo Dashboard"),
        ..Default::default()
    };

    eframe::run_native(
        "Git Repo Dashboard",
        options,
        Box::new(|_cc| Box::new(GitApp::default()) as Box<dyn eframe::App>),
    )
}
