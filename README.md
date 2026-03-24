# Git Repo Dashboard

A fast, lightweight desktop app for scanning and monitoring multiple Git repositories at once. Built with Rust using `egui`/`eframe`, it gives you a clear overview of repo status (ahead, behind, diverged, etc.) across a directory tree.

## Features
* Scan a root folder for repositories (depth 2)
* Parallel scanning using Rayon for speed
* Fetch all remotes across repos with one click
* Status overview:
  * Current
  * Ahead
  * Behind
  * Diverged
  * No Upstream
* Smart grouping by folder
* Detects GitHub repos automatically
* Option to hide non-Git folders
* Click-to-open folders in your OS file manager
* Color-coded statuses for quick visual feedback

## UI overview
* Top bar: Select folder, refresh, fetch all
* Summary counts: Quick counts of repo states
* Grouped view: Repos organized by parent folder
* Status table:
  * Folder name (clickable)
  * Git presence
  * GitHub detection
  * Status (color-coded)

## Usage
1. Click 📂 Select Folder
2. Choose a root directory containing projects
3. Browse grouped repositories
4. Use: <br>
  🔄 Refresh to rescan <br>
  ⬇ Fetch All to update remotes 
5. Click any repo name to open it

## Status overview

