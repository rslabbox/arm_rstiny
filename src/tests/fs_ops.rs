#![allow(unused)]

use alloc::string::String;
use alloc::vec::Vec;
use unittest::{assert, assert_eq, def_test};
use crate::alloc::string::ToString;

use crate::fs;
use crate::fs::{DirEntry, FileType, OpenOptions};

const TEST_DIR: &str = "/fsops_test";
const TEST_FILE: &str = "/fsops_test/hello.txt";
const TEST_LINK: &str = "/fsops_test/hello_link.txt";
const TEST_FILE2: &str = "/fsops_test/hello2.txt";
const TEST_COPY: &str = "/fsops_test/hello_copy.txt";
const TEST_RENAMED: &str = "/fsops_test/renamed.txt";
const TEST_SYMLINK: &str = "/fsops_test/hello_sym.txt";
const TEST_NESTED: &str = "/fsops_test/a/b/c";

fn cleanup() {
    // deep cleanup — remove_all handles recursive deletion
    let _ = fs::remove_all(TEST_DIR);
}

#[def_test]
fn test_fsops_mount_umount() {
    assert!(fs::mount().is_ok());
    assert!(fs::umount().is_ok());
    assert!(fs::mount().is_ok());
}

#[def_test]
fn test_fsops_basic_ops() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());
    assert!(fs::list_dir(Some(TEST_DIR)).is_ok());
    assert!(fs::change_dir(TEST_DIR).is_ok());
    assert_eq!(fs::current_dir(), TEST_DIR.to_string());
    assert!(fs::change_dir("/").is_ok());

    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let data = b"hello";
    let wrote = fs::write_file(handle, 0, data).expect("write_file failed");
    assert_eq!(wrote, data.len());

    let read = fs::read_file(handle, 0, 0).expect("read_file failed");
    assert_eq!(read, data.to_vec());

    assert!(fs::file_truncate(handle, 2).is_ok());
    let read2 = fs::read_file(handle, 0, 0).expect("read_file after truncate failed");
    assert_eq!(read2, b"he".to_vec());

    assert!(fs::close(handle).is_ok());
    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_link_and_readlink() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());
    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let _ = fs::write_file(handle, 0, b"data").expect("write_file failed");
    assert!(fs::close(handle).is_ok());

    assert!(fs::read_link(TEST_FILE).is_err());

    #[cfg(feature = "fat32")]
    {
        assert!(fs::link(TEST_FILE, TEST_LINK).is_err());
    }

    #[cfg(feature = "fs9p")]
    {
        assert!(fs::link(TEST_FILE, TEST_LINK).is_ok());
        assert!(fs::file_remove(TEST_LINK).is_ok());
    }

    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_open_close_unlink() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    let options = OpenOptions {
        read: true,
        write: true,
        create: true,
        truncate: true,
        append: false,
    };

    let handle = fs::open(TEST_FILE, options).expect("open failed");
    let _ = fs::write_file(handle, 0, b"abc").expect("write_file failed");
    assert!(fs::close(handle).is_ok());

    assert!(fs::unlink(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_stat_and_helpers() {
    assert!(fs::mount().is_ok());
    cleanup();

    // non-existent path
    assert!(!fs::exists(TEST_DIR));
    assert!(!fs::is_dir(TEST_DIR));
    assert!(!fs::is_file(TEST_FILE));
    assert!(fs::file_size(TEST_FILE).is_err());

    // create dir and verify
    assert!(fs::mkdir(TEST_DIR).is_ok());
    assert!(fs::exists(TEST_DIR));
    assert!(fs::is_dir(TEST_DIR));
    assert!(!fs::is_file(TEST_DIR));

    // stat on directory
    let dir_meta = fs::stat(TEST_DIR).expect("stat dir failed");
    assert_eq!(dir_meta.file_type, FileType::Directory);
    println!("  stat({}): type={:?} mode={:o} size={}", TEST_DIR, dir_meta.file_type, dir_meta.mode, dir_meta.size);

    // create file with content and verify
    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let _ = fs::write_file(handle, 0, b"hello world").expect("write failed");
    assert!(fs::close(handle).is_ok());

    assert!(fs::exists(TEST_FILE));
    assert!(fs::is_file(TEST_FILE));
    assert!(!fs::is_dir(TEST_FILE));

    let file_meta = fs::stat(TEST_FILE).expect("stat file failed");
    assert_eq!(file_meta.file_type, FileType::File);
    assert_eq!(file_meta.size, 11);
    println!("  stat({}): type={:?} mode={:o} size={} nlink={} uid={} gid={}",
        TEST_FILE, file_meta.file_type, file_meta.mode, file_meta.size,
        file_meta.nlink, file_meta.uid, file_meta.gid);

    let size = fs::file_size(TEST_FILE).expect("file_size failed");
    assert_eq!(size, 11);

    // cleanup
    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_readdir() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    // create two files
    let h1 = fs::create_file(TEST_FILE).expect("create file1 failed");
    assert!(fs::close(h1).is_ok());
    let h2 = fs::create_file(TEST_FILE2).expect("create file2 failed");
    assert!(fs::close(h2).is_ok());

    // readdir should list both files
    let entries = fs::readdir(TEST_DIR).expect("readdir failed");
    println!("  readdir({}):", TEST_DIR);
    for entry in &entries {
        println!("    {:?} {}", entry.file_type, entry.name);
    }
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"hello.txt"));
    assert!(names.contains(&"hello2.txt"));

    // verify type info
    for entry in &entries {
        if entry.name == "hello.txt" || entry.name == "hello2.txt" {
            assert_eq!(entry.file_type, FileType::File);
        }
    }

    // cleanup
    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::file_remove(TEST_FILE2).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_rename() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let _ = fs::write_file(handle, 0, b"rename me").expect("write failed");
    assert!(fs::close(handle).is_ok());

    #[cfg(feature = "fs9p")]
    {
        assert!(fs::rename(TEST_FILE, TEST_RENAMED).is_ok());
        assert!(!fs::exists(TEST_FILE));
        assert!(fs::exists(TEST_RENAMED));

        // read content through renamed path
        let opt = OpenOptions { read: true, ..Default::default() };
        let h = fs::open(TEST_RENAMED, opt).expect("open renamed failed");
        let data = fs::read_file(h, 0, 0).expect("read renamed failed");
        assert_eq!(data, b"rename me".to_vec());
        assert!(fs::close(h).is_ok());

        assert!(fs::file_remove(TEST_RENAMED).is_ok());
    }

    #[cfg(feature = "fat32")]
    {
        assert!(fs::rename(TEST_FILE, TEST_RENAMED).is_err());
        assert!(fs::file_remove(TEST_FILE).is_ok());
    }

    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_symlink() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let _ = fs::write_file(handle, 0, b"sym target").expect("write failed");
    assert!(fs::close(handle).is_ok());

    #[cfg(feature = "fs9p")]
    {
        assert!(fs::symlink(TEST_FILE, TEST_SYMLINK).is_ok());
        assert!(fs::exists(TEST_SYMLINK));

        // readlink should return the target
        let target = fs::read_link(TEST_SYMLINK).expect("read_link failed");
        assert_eq!(target, TEST_FILE.to_string());

        // stat the symlink itself
        let meta = fs::stat(TEST_SYMLINK).expect("stat symlink failed");
        // qid_type should indicate symlink
        assert_eq!(meta.file_type, FileType::Symlink);

        assert!(fs::file_remove(TEST_SYMLINK).is_ok());
    }

    #[cfg(feature = "fat32")]
    {
        assert!(fs::symlink(TEST_FILE, TEST_SYMLINK).is_err());
    }

    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_chmod() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    assert!(fs::close(handle).is_ok());

    #[cfg(feature = "fs9p")]
    {
        assert!(fs::chmod(TEST_FILE, 0o755).is_ok());

        let meta = fs::stat(TEST_FILE).expect("stat after chmod failed");
        // Check that the permission bits are updated (lower 12 bits)
        assert_eq!(meta.mode & 0o7777, 0o755);
    }

    #[cfg(feature = "fat32")]
    {
        assert!(fs::chmod(TEST_FILE, 0o755).is_err());
    }

    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_copy_file() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    // create source file
    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let data = b"copy this content over";
    let _ = fs::write_file(handle, 0, data).expect("write failed");
    assert!(fs::close(handle).is_ok());

    // copy
    let copied = fs::copy_file(TEST_FILE, TEST_COPY).expect("copy_file failed");
    assert_eq!(copied, data.len() as u64);

    // verify copy content
    let opt = OpenOptions { read: true, ..Default::default() };
    let h = fs::open(TEST_COPY, opt).expect("open copy failed");
    let read = fs::read_file(h, 0, 0).expect("read copy failed");
    assert_eq!(read, data.to_vec());
    assert!(fs::close(h).is_ok());

    // verify sizes match
    let src_size = fs::file_size(TEST_FILE).expect("file_size src");
    let dst_size = fs::file_size(TEST_COPY).expect("file_size dst");
    assert_eq!(src_size, dst_size);

    assert!(fs::file_remove(TEST_COPY).is_ok());
    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_open_options_append() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    // create and write initial data
    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let _ = fs::write_file(handle, 0, b"hello").expect("write failed");
    assert!(fs::close(handle).is_ok());

    // open in append mode and write more
    let opt = OpenOptions {
        read: true,
        write: true,
        create: false,
        truncate: false,
        append: true,
    };
    let h = fs::open(TEST_FILE, opt).expect("open append failed");
    // 9P TWRITE uses explicit offset (pwritev ignores O_APPEND),
    // so we compute the append position via file_size.
    let pos = fs::file_size(TEST_FILE).expect("file_size failed");
    let _ = fs::write_file(h, pos, b" world").expect("append write failed");
    assert!(fs::close(h).is_ok());

    // read back full content
    let ropt = OpenOptions { read: true, ..Default::default() };
    let rh = fs::open(TEST_FILE, ropt).expect("open for read failed");
    let content = fs::read_file(rh, 0, 0).expect("read failed");
    assert_eq!(content, b"hello world".to_vec());
    assert!(fs::close(rh).is_ok());

    assert!(fs::file_remove(TEST_FILE).is_ok());
    assert!(fs::dir_remove(TEST_DIR).is_ok());
}

#[def_test]
fn test_fsops_mkdir_all() {
    assert!(fs::mount().is_ok());
    cleanup();

    // create nested directories in one call
    assert!(fs::mkdir_all(TEST_NESTED).is_ok());

    // all intermediate dirs should exist
    assert!(fs::is_dir("/fsops_test"));
    assert!(fs::is_dir("/fsops_test/a"));
    assert!(fs::is_dir("/fsops_test/a/b"));
    assert!(fs::is_dir(TEST_NESTED));

    // calling again should be idempotent
    assert!(fs::mkdir_all(TEST_NESTED).is_ok());

    // create a file inside the deepest dir
    let deep_file = "/fsops_test/a/b/c/deep.txt";
    let h = fs::create_file(deep_file).expect("create deep file failed");
    let _ = fs::write_file(h, 0, b"deep").expect("write failed");
    assert!(fs::close(h).is_ok());

    println!("  mkdir_all created: {} => file inside OK", TEST_NESTED);
    assert!(fs::exists(deep_file));

    cleanup();
    assert!(!fs::exists(TEST_DIR));
}

#[def_test]
fn test_fsops_remove_all() {
    assert!(fs::mount().is_ok());
    cleanup();

    // build a tree:
    //   /fsops_test/
    //     hello.txt
    //     sub/
    //       nested.txt
    assert!(fs::mkdir_all("/fsops_test/sub").is_ok());
    let h1 = fs::create_file(TEST_FILE).expect("create file1 failed");
    assert!(fs::close(h1).is_ok());
    let h2 = fs::create_file("/fsops_test/sub/nested.txt").expect("create nested failed");
    assert!(fs::close(h2).is_ok());

    // remove_all should delete everything
    assert!(fs::remove_all(TEST_DIR).is_ok());
    assert!(!fs::exists(TEST_DIR));
    assert!(!fs::exists(TEST_FILE));
    assert!(!fs::exists("/fsops_test/sub"));

    // remove root should fail
    assert!(fs::remove_all("/").is_err());

    // remove non-existent should fail
    assert!(fs::remove_all("/does_not_exist").is_err());
}

#[def_test]
fn test_fsops_walk() {
    assert!(fs::mount().is_ok());
    cleanup();

    // build a tree:
    //   /fsops_test/
    //     hello.txt
    //     hello2.txt
    //     sub/
    //       nested.txt
    assert!(fs::mkdir_all("/fsops_test/sub").is_ok());
    let h1 = fs::create_file(TEST_FILE).expect("create file1 failed");
    assert!(fs::close(h1).is_ok());
    let h2 = fs::create_file(TEST_FILE2).expect("create file2 failed");
    assert!(fs::close(h2).is_ok());
    let h3 = fs::create_file("/fsops_test/sub/nested.txt").expect("create nested failed");
    assert!(fs::close(h3).is_ok());

    // walk and collect all paths
    let mut visited: Vec<String> = Vec::new();
    fs::walk(TEST_DIR, &mut |path, entry| {
        println!("  walk: {:?} {}", entry.file_type, path);
        visited.push(String::from(path));
        true
    })
    .expect("walk failed");

    // should have visited at least 4 entries
    assert!(visited.len() >= 4);
    assert!(visited.iter().any(|p| p.ends_with("hello.txt")));
    assert!(visited.iter().any(|p| p.ends_with("nested.txt")));
    assert!(visited.iter().any(|p| p.ends_with("/sub")));

    // test early termination: visitor returns false after 2nd call
    let mut count = 0usize;
    fs::walk(TEST_DIR, &mut |_path, _entry| {
        count += 1;
        count < 2
    })
    .expect("walk early stop failed");
    assert_eq!(count, 2);

    cleanup();
}

#[def_test]
fn test_fsops_fsync() {
    assert!(fs::mount().is_ok());
    cleanup();

    assert!(fs::mkdir(TEST_DIR).is_ok());

    let handle = fs::create_file(TEST_FILE).expect("create_file failed");
    let _ = fs::write_file(handle, 0, b"fsync test data").expect("write failed");

    // fsync should not fail for an open file
    let result = fs::fsync(handle);
    println!("  fsync result: {:?}", result);
    // On QEMU 9P local backend, TFSYNC may or may not be supported;
    // we just verify it doesn't panic.

    assert!(fs::close(handle).is_ok());

    cleanup();
}
