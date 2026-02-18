use rfd::FileDialog;
use std::path::PathBuf;
use std::{fs, io, path::Path};

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

fn listdir(path: impl AsRef<Path>) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(path)?
        .map(|res| {
            // Convert each DirEntry into a plain filename string
            let entry = res?;
            let name = entry
                .file_name()                     // OsString
                .to_string_lossy()               // handle non-UTF8 nicely
                .into_owned();                   // String
            Ok(name)
        })
        .collect::<io::Result<Vec<_>>>()?;
    // Optional: sort to make results deterministic (Python does not sort by default)
    names.sort();
    Ok(names)
}

fn main() -> io::Result<()> {
    let selected = select_folder();

    if let Some(path) = selected {
        println!("Main received: {}", path.display());
    }

    let files = listdir()?;
    println!("{files:#?}");
    Ok(())
}

