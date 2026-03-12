use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) fn cleanup_unselected_placeholder_files(
    output_folder: &str,
    selected_indices: &[usize],
    file_entries: &[(usize, String)],
) {
    if selected_indices.is_empty() {
        return;
    }

    let selected: HashSet<usize> = selected_indices.iter().copied().collect();
    let base = Path::new(output_folder);
    let mut parent_dirs: Vec<PathBuf> = Vec::new();

    for (idx, relative_path) in file_entries {
        if selected.contains(idx) {
            continue;
        }

        let full_path = base.join(relative_path);
        if let Ok(meta) = std::fs::metadata(&full_path) {
            if meta.is_file() {
                let _ = std::fs::remove_file(&full_path);
                if let Some(parent) = Path::new(relative_path).parent() {
                    if !parent.as_os_str().is_empty() {
                        parent_dirs.push(parent.to_path_buf());
                    }
                }
            }
        }
    }

    parent_dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    parent_dirs.dedup();

    for relative_dir in parent_dirs {
        let _ = std::fs::remove_dir(base.join(relative_dir));
    }
}
