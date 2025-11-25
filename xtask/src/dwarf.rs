use crate::utils::TaskResult;
use std::path::Path;
use xshell::{Shell, cmd};

const DEBUG_SECTIONS: &[&str] = &[
    "debug_abbrev",
    "debug_addr",
    "debug_aranges",
    "debug_info",
    "debug_line",
    "debug_line_str",
    "debug_ranges",
    "debug_rnglists",
    "debug_str",
    "debug_str_offsets",
];

/// Process debug sections in the ELF file to make them accessible at runtime.
///
/// This function:
/// 1. Extracts all .debug_* sections to temporary files
/// 2. Strips debug sections from the ELF
/// 3. Re-injects them as debug_* (without dot prefix) sections
/// 4. Cleans up temporary files
///
/// This allows the linker script to reference these sections with proper symbols
/// (__start_debug_* and __stop_debug_*) that can be accessed at runtime.
pub fn process_debug_sections(elf_path: &Path) -> TaskResult<()> {
    let sh = Shell::new()?;
    let elf_dir = elf_path
        .parent()
        .ok_or_else(|| crate::utils::TaskError::NotFound(elf_path.to_string_lossy().to_string()))?;

    sh.change_dir(elf_dir);

    let elf_name = elf_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| crate::utils::TaskError::NotFound(elf_path.to_string_lossy().to_string()))?;

    info!("    Extracting debug sections from {}", elf_name);

    // Step 1: Extract all .debug_* sections to temporary files
    for section in DEBUG_SECTIONS {
        let section_file = format!("{}.bin", section);
        let dotted_section = format!(".{}", section);

        // Try to dump the section, if it doesn't exist, create an empty file
        let result = cmd!(
            sh,
            "rust-objcopy {elf_name} --dump-section {dotted_section}={section_file}"
        )
        .ignore_stderr()
        .run();

        if result.is_err() {
            // Section doesn't exist, create empty file
            std::fs::write(elf_dir.join(&section_file), &[])?;
        }
    }

    info!("    Stripping debug sections from {}", elf_name);

    // Step 2: Strip all debug sections from the ELF
    cmd!(sh, "rust-objcopy {elf_name} --strip-debug").run()?;

    info!("    Re-injecting debug sections into {}", elf_name);

    // Step 3: Re-inject sections without dot prefix
    // Build the command with all --update-section and --rename-section arguments
    let mut objcopy_cmd = vec!["rust-objcopy".to_string(), elf_name.to_string()];

    for section in DEBUG_SECTIONS {
        let section_file = format!("{}.bin", section);
        objcopy_cmd.push("--update-section".to_string());
        objcopy_cmd.push(format!("{}={}", section, section_file));
        objcopy_cmd.push("--rename-section".to_string());
        objcopy_cmd.push(format!("{}=.{}", section, section));
    }

    // Execute the command
    let mut cmd = std::process::Command::new(&objcopy_cmd[0]);
    cmd.args(&objcopy_cmd[1..]);
    cmd.current_dir(elf_dir);

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::utils::TaskError::Other(format!(
            "Failed to re-inject debug sections: {}",
            stderr
        )));
    }

    info!("    Cleaning up temporary files");

    // Step 4: Clean up temporary files
    for section in DEBUG_SECTIONS {
        let section_file = elf_dir.join(format!("{}.bin", section));
        if section_file.exists() {
            std::fs::remove_file(section_file)?;
        }
    }

    Ok(())
}
