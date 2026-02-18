use rfd::FileDialog;
use std::path::PathBuf;

fn select_folder() -> Option<PathBuf> {
    let folder = FileDialog::new()
        .set_title("Select a folder")
        .pick_folder();

    match folder {
        Some(path) => {
            // println!("Selected folder: {}", path.display());
            Some(path) // return it
        }
        None => {
            // println!("No folder selected.");
            None // return None
        }
    }
}

fn main() {
    let selected = select_folder();

    if let Some(path) = selected {
        println!("Main received: {}", path.display());
    }
}
