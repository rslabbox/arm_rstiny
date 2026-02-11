//! Filesystem commands.

use crate::TinyResult;
use crate::user::Command;
use crate::user::CommandContext;
use crate::fs;
use crate::fs::{FileType, OpenOptions};

/// List directory contents.
pub static LS: LsCommand = LsCommand;

pub struct LsCommand;

impl Command for LsCommand {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn description(&self) -> &'static str {
        "List directory contents"
    }

    fn usage(&self) -> &'static str {
        "Usage: ls [-l] [path]"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let mut long = false;
        let mut path: Option<&str> = None;
        for arg in ctx.args.iter() {
            if arg == "-l" {
                long = true;
            } else if path.is_none() {
                path = Some(arg);
            }
        }
        if long {
            let target = path.unwrap_or(".");
            match fs::readdir(target) {
                Ok(entries) => {
                    for entry in entries {
                        let child = if target == "/" || target == "." {
                            alloc::format!("{}/{}", fs::current_dir().trim_end_matches('/'), entry.name)
                        } else {
                            let base = fs::current_dir();
                            let full = if target.starts_with('/') {
                                alloc::string::String::from(target)
                            } else if base.ends_with('/') {
                                alloc::format!("{}{}", base, target)
                            } else {
                                alloc::format!("{}/{}", base, target)
                            };
                            alloc::format!("{}/{}", full.trim_end_matches('/'), entry.name)
                        };
                        let type_char = match entry.file_type {
                            FileType::Directory => 'd',
                            FileType::Symlink => 'l',
                            FileType::File => '-',
                            FileType::Other => '?',
                        };
                        if let Ok(meta) = fs::stat(&child) {
                            println!("{}{:o}  {:>8}  {}", type_char, meta.mode & 0o7777, meta.size, entry.name);
                        } else {
                            println!("{}           {}", type_char, entry.name);
                        }
                    }
                }
                Err(e) => println!("ls: {}", e),
            }
        } else {
            if let Err(e) = fs::list_dir(path) {
                println!("ls: {}", e);
            }
        }
        Ok(())
    }
}

/// Change directory.
pub static CD: CdCommand = CdCommand;

pub struct CdCommand;

impl Command for CdCommand {
    fn name(&self) -> &'static str {
        "cd"
    }

    fn description(&self) -> &'static str {
        "Change current working directory"
    }

    fn usage(&self) -> &'static str {
        "Usage: cd <path>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        if let Some(path) = ctx.args.get(0) {
            if let Err(e) = fs::change_dir(path) {
                println!("cd: {}", e);
            }
        } else {
            println!("Usage: cd <path>");
        }
        Ok(())
    }
}

/// Make directory.
pub static MKDIR: MkdirCommand = MkdirCommand;

pub struct MkdirCommand;

impl Command for MkdirCommand {
    fn name(&self) -> &'static str {
        "mkdir"
    }

    fn description(&self) -> &'static str {
        "Create a directory"
    }

    fn usage(&self) -> &'static str {
        "Usage: mkdir [-p] <path>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let mut recursive = false;
        let mut path: Option<&str> = None;
        for arg in ctx.args.iter() {
            if arg == "-p" {
                recursive = true;
            } else if path.is_none() {
                path = Some(arg);
            }
        }
        match path {
            Some(p) => {
                let result = if recursive { fs::mkdir_all(p) } else { fs::mkdir(p) };
                if let Err(e) = result {
                    println!("mkdir: {}", e);
                }
            }
            None => println!("Usage: mkdir [-p] <path>"),
        }
        Ok(())
    }
}


/// Print current working directory.
pub static PWD: PwdCommand = PwdCommand;

pub struct PwdCommand;

impl Command for PwdCommand {
    fn name(&self) -> &'static str {
        "pwd"
    }

    fn description(&self) -> &'static str {
        "Print current working directory"
    }

    fn usage(&self) -> &'static str {
        "Usage: pwd"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, _ctx: &CommandContext) -> TinyResult<()> {
        let cwd = fs::current_dir();
        println!("{}", cwd);
        Ok(())
    }
}

/// Display file contents.
pub static CAT: CatCommand = CatCommand;

pub struct CatCommand;

impl Command for CatCommand {
    fn name(&self) -> &'static str {
        "cat"
    }

    fn description(&self) -> &'static str {
        "Display file contents"
    }

    fn usage(&self) -> &'static str {
        "Usage: cat <file>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let path = match ctx.args.get(0) {
            Some(p) => p,
            None => { println!("Usage: cat <file>"); return Ok(()); }
        };
        let opt = OpenOptions { read: true, ..Default::default() };
        match fs::open(path, opt) {
            Ok(handle) => {
                let mut offset = 0u64;
                loop {
                    match fs::read_file(handle, offset, 4096) {
                        Ok(data) => {
                            if data.is_empty() { break; }
                            if let Ok(text) = core::str::from_utf8(&data) {
                                print!("{}", text);
                            } else {
                                println!("cat: binary file, {} bytes at offset {}", data.len(), offset);
                                break;
                            }
                            offset += data.len() as u64;
                        }
                        Err(e) => { println!("cat: read error: {}", e); break; }
                    }
                }
                let _ = fs::close(handle);
                println!();
            }
            Err(e) => println!("cat: {}", e),
        }
        Ok(())
    }
}

/// Create an empty file or update timestamps.
pub static TOUCH: TouchCommand = TouchCommand;

pub struct TouchCommand;

impl Command for TouchCommand {
    fn name(&self) -> &'static str {
        "touch"
    }

    fn description(&self) -> &'static str {
        "Create an empty file"
    }

    fn usage(&self) -> &'static str {
        "Usage: touch <file>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let path = match ctx.args.get(0) {
            Some(p) => p,
            None => { println!("Usage: touch <file>"); return Ok(()); }
        };
        if fs::exists(path) {
            return Ok(());
        }
        match fs::create_file(path) {
            Ok(handle) => { let _ = fs::close(handle); }
            Err(e) => println!("touch: {}", e),
        }
        Ok(())
    }
}

/// Remove files or directories.
pub static RM: RmCommand = RmCommand;

pub struct RmCommand;

impl Command for RmCommand {
    fn name(&self) -> &'static str {
        "rm"
    }

    fn description(&self) -> &'static str {
        "Remove files or directories"
    }

    fn usage(&self) -> &'static str {
        "Usage: rm [-r] <path>..."
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let mut recursive = false;
        let mut paths = alloc::vec::Vec::new();
        for arg in ctx.args.iter() {
            if arg == "-r" || arg == "-rf" {
                recursive = true;
            } else {
                paths.push(arg);
            }
        }
        if paths.is_empty() {
            println!("Usage: rm [-r] <path>...");
            return Ok(());
        }
        for path in paths {
            let result = if recursive {
                fs::remove_all(path)
            } else if fs::is_dir(path) {
                println!("rm: {}: is a directory (use -r)", path);
                continue;
            } else {
                fs::file_remove(path)
            };
            if let Err(e) = result {
                println!("rm: {}: {}", path, e);
            }
        }
        Ok(())
    }
}

/// Remove empty directory.
pub static RMDIR: RmdirCommand = RmdirCommand;

pub struct RmdirCommand;

impl Command for RmdirCommand {
    fn name(&self) -> &'static str {
        "rmdir"
    }

    fn description(&self) -> &'static str {
        "Remove empty directories"
    }

    fn usage(&self) -> &'static str {
        "Usage: rmdir <dir>..."
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        if ctx.args.is_empty() {
            println!("Usage: rmdir <dir>...");
            return Ok(());
        }
        for arg in ctx.args.iter() {
            if let Err(e) = fs::dir_remove(arg) {
                println!("rmdir: {}: {}", arg, e);
            }
        }
        Ok(())
    }
}

/// Copy files.
pub static CP: CpCommand = CpCommand;

pub struct CpCommand;

impl Command for CpCommand {
    fn name(&self) -> &'static str {
        "cp"
    }

    fn description(&self) -> &'static str {
        "Copy a file"
    }

    fn usage(&self) -> &'static str {
        "Usage: cp <source> <destination>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let (src, dst) = match (ctx.args.get(0), ctx.args.get(1)) {
            (Some(s), Some(d)) => (s, d),
            _ => { println!("Usage: cp <source> <destination>"); return Ok(()); }
        };
        match fs::copy_file(src, dst) {
            Ok(bytes) => println!("{} bytes copied", bytes),
            Err(e) => println!("cp: {}", e),
        }
        Ok(())
    }
}

/// Move / rename files.
pub static MV: MvCommand = MvCommand;

pub struct MvCommand;

impl Command for MvCommand {
    fn name(&self) -> &'static str {
        "mv"
    }

    fn description(&self) -> &'static str {
        "Move or rename a file"
    }

    fn usage(&self) -> &'static str {
        "Usage: mv <source> <destination>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let (src, dst) = match (ctx.args.get(0), ctx.args.get(1)) {
            (Some(s), Some(d)) => (s, d),
            _ => { println!("Usage: mv <source> <destination>"); return Ok(()); }
        };
        if let Err(e) = fs::rename(src, dst) {
            println!("mv: {}", e);
        }
        Ok(())
    }
}

/// Create links.
pub static LN: LnCommand = LnCommand;

pub struct LnCommand;

impl Command for LnCommand {
    fn name(&self) -> &'static str {
        "ln"
    }

    fn description(&self) -> &'static str {
        "Create a link to a file"
    }

    fn usage(&self) -> &'static str {
        "Usage: ln [-s] <target> <link_name>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let mut symbolic = false;
        let mut positional = alloc::vec::Vec::new();
        for arg in ctx.args.iter() {
            if arg == "-s" {
                symbolic = true;
            } else {
                positional.push(arg);
            }
        }
        let (target, link_name) = match (positional.get(0), positional.get(1)) {
            (Some(t), Some(l)) => (*t, *l),
            _ => { println!("Usage: ln [-s] <target> <link_name>"); return Ok(()); }
        };
        let result = if symbolic {
            fs::symlink(target, link_name)
        } else {
            fs::link(target, link_name)
        };
        if let Err(e) = result {
            println!("ln: {}", e);
        }
        Ok(())
    }
}

/// Display file status.
pub static STAT: StatCommand = StatCommand;

pub struct StatCommand;

impl Command for StatCommand {
    fn name(&self) -> &'static str {
        "stat"
    }

    fn description(&self) -> &'static str {
        "Display file or directory status"
    }

    fn usage(&self) -> &'static str {
        "Usage: stat <path>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let path = match ctx.args.get(0) {
            Some(p) => p,
            None => { println!("Usage: stat <path>"); return Ok(()); }
        };
        match fs::stat(path) {
            Ok(meta) => {
                let type_str = match meta.file_type {
                    FileType::File => "regular file",
                    FileType::Directory => "directory",
                    FileType::Symlink => "symbolic link",
                    FileType::Other => "other",
                };
                println!("  File: {}", path);
                println!("  Type: {}", type_str);
                println!("  Size: {}  Links: {}", meta.size, meta.nlink);
                println!("  Mode: {:o}  Uid: {}  Gid: {}", meta.mode & 0o7777, meta.uid, meta.gid);
                if let Ok(target) = fs::read_link(path) {
                    println!("  Link: -> {}", target);
                }
            }
            Err(e) => println!("stat: {}: {}", path, e),
        }
        Ok(())
    }
}

/// Change file mode.
pub static CHMOD: ChmodCommand = ChmodCommand;

pub struct ChmodCommand;

impl Command for ChmodCommand {
    fn name(&self) -> &'static str {
        "chmod"
    }

    fn description(&self) -> &'static str {
        "Change file permissions"
    }

    fn usage(&self) -> &'static str {
        "Usage: chmod <mode> <file>  (mode in octal, e.g. 755)"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let (mode_str, path) = match (ctx.args.get(0), ctx.args.get(1)) {
            (Some(m), Some(p)) => (m, p),
            _ => { println!("Usage: chmod <mode> <file>"); return Ok(()); }
        };
        let mode = match u32::from_str_radix(mode_str, 8) {
            Ok(m) => m,
            Err(_) => { println!("chmod: invalid mode: {}", mode_str); return Ok(()); }
        };
        if let Err(e) = fs::chmod(path, mode) {
            println!("chmod: {}", e);
        }
        Ok(())
    }
}

/// Display directory tree.
pub static TREE: TreeCommand = TreeCommand;

pub struct TreeCommand;

impl Command for TreeCommand {
    fn name(&self) -> &'static str {
        "tree"
    }

    fn description(&self) -> &'static str {
        "Display directory tree"
    }

    fn usage(&self) -> &'static str {
        "Usage: tree [path]"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let path = ctx.args.get(0).unwrap_or(".");
        println!("{}", path);
        let mut dirs = 0usize;
        let mut files = 0usize;
        if let Err(e) = fs::walk(path, &mut |child_path, entry| {
            // Calculate depth for indentation
            let base = if path == "." || path == "/" {
                fs::current_dir()
            } else {
                alloc::string::String::from(path)
            };
            let relative = child_path.strip_prefix(base.trim_end_matches('/')).unwrap_or(child_path);
            let depth = relative.matches('/').count();
            for _ in 1..depth {
                print!("    ");
            }
            let name = child_path.rsplit('/').next().unwrap_or(child_path);
            match entry.file_type {
                FileType::Directory => {
                    println!("|-- {}/", name);
                    dirs += 1;
                }
                FileType::Symlink => {
                    if let Ok(target) = fs::read_link(child_path) {
                        println!("|-- {} -> {}", name, target);
                    } else {
                        println!("|-- {}@", name);
                    }
                    files += 1;
                }
                _ => {
                    println!("|-- {}", name);
                    files += 1;
                }
            }
            true
        }) {
            println!("tree: {}", e);
        }
        println!("\n{} directories, {} files", dirs, files);
        Ok(())
    }
}

/// Write text to a file.
pub static WRITE: WriteCommand = WriteCommand;

pub struct WriteCommand;

impl Command for WriteCommand {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write text to a file (overwrites)"
    }

    fn usage(&self) -> &'static str {
        "Usage: write <file> <text...>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let path = match ctx.args.get(0) {
            Some(p) => p,
            None => { println!("Usage: write <file> <text...>"); return Ok(()); }
        };
        // Everything after the first arg is the content
        let content = match ctx.args_raw.strip_prefix(path) {
            Some(rest) => rest.trim_start(),
            None => { println!("Usage: write <file> <text...>"); return Ok(()); }
        };
        if content.is_empty() {
            println!("write: nothing to write");
            return Ok(());
        }
        match fs::create_file(path) {
            Ok(handle) => {
                match fs::write_file(handle, 0, content.as_bytes()) {
                    Ok(n) => println!("{} bytes written", n),
                    Err(e) => println!("write: {}", e),
                }
                let _ = fs::close(handle);
            }
            Err(e) => println!("write: {}", e),
        }
        Ok(())
    }
}
