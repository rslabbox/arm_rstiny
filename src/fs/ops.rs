use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use crate::fs::{FILESYSTEM, CWD};

pub fn resolve_path(path: &str) -> String {
    let cwd = CWD.lock().clone();
    let abs_path = if path.starts_with('/') {
        String::from(path)
    } else {
        if cwd.ends_with('/') {
            format!("{}{}", cwd, path)
        } else {
            format!("{}/{}", cwd, path)
        }
    };

    let mut parts = Vec::new();
    for part in abs_path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }

    let mut res = String::from("/");
    res.push_str(&parts.join("/"));
    res
}

pub fn list_dir(path: Option<&str>) -> Result<(), String> {
    let fs_guard = FILESYSTEM.lock();
    let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
    
    let target_path = resolve_path(path.unwrap_or(""));
    // fatfs expects paths without leading '/' usually, unless it treats it as root. 
    // root_dir() is root.
    let root = fs.root_dir();
    let dir = if target_path == "/" {
        root
    } else {
        // Strip leading / for fatfs
        let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
        root.open_dir(rel_path).map_err(|e| format!("Failed to open dir: {:?}", e))?
    };

    for entry in dir.iter() {
        let e = entry.map_err(|e| format!("Error reading entry: {:?}", e))?;
        println!("{}", e.file_name());
    }
    Ok(())
}

pub fn change_dir(path: &str) -> Result<(), String> {
    let fs_guard = FILESYSTEM.lock();
    let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;

    let target_path = resolve_path(path);
    let root = fs.root_dir();
    
    if target_path != "/" {
        let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
        // Verify it exists and is a dir
        let dir = root.open_dir(rel_path).map_err(|_| format!("Directory not found: {}", target_path))?;
        // Just verify it opens
        drop(dir);
    }

    *CWD.lock() = target_path;
    Ok(())
}

pub fn make_dir(path: &str) -> Result<(), String> {
    let fs_guard = FILESYSTEM.lock();
    let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
    
    let target_path = resolve_path(path);
    if target_path == "/" {
        return Err(String::from("Cannot create root directory"));
    }
    
    let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
    fs.root_dir().create_dir(rel_path).map_err(|e| format!("Failed to create dir: {:?}", e))?;
    Ok(())
}

pub fn current_dir() -> String {
    CWD.lock().clone()
}
