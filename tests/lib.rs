extern crate libarchive;

pub mod util;

use libarchive::archive;
use libarchive::reader::{self};
use libarchive::writer;
use std::fs::File;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn assert_string(string: &str) {
    assert_eq!(string, "hello, world!\n");
}

fn assert_fixture(tempdir: &tempfile::TempDir) {
    assert_string(
        std::fs::read_to_string(tempdir.path().join("hello.txt"))
            .unwrap()
            .as_str(),
    );
}

#[test]
fn reading_from_file() {
    let tar = util::path::fixture("sample.tar.gz");
    let mut reader = reader::Builder::new()
        .support_all()
        .unwrap()
        .open_file(tar)
        .unwrap();

    reader.next_header();
    // let entry: &archive::Entry = &reader.entry;
    // println!("{:?}", entry.pathname());
    // println!("{:?}", entry.size());
    // for entry in reader.entries() {
    //     let file = entry as &archive::Entry;
    //     println!("{:?}", file.pathname());
    //     println!("{:?}", file.size());
    // }
    assert_eq!(4, 4);
}

#[test]
fn read_archive_from_stream() {
    let tar = util::path::fixture("sample.tar.gz");
    let f = File::open(tar).ok().unwrap();
    let builder = reader::Builder::new().support_all().unwrap();

    match builder.open_stream(f) {
        Ok(mut reader) => {
            assert_eq!(reader.header_position(), 0);
            let writer = writer::Disk::new();

            let tempfile = tempfile::tempdir().unwrap();
            let count = writer.write(&mut reader, tempfile.path().to_str()).unwrap();
            assert_eq!(count, 14);
            assert_eq!(reader.header_position(), 1024);
            assert_fixture(&tempfile);
            assert_eq!(4, 4);
        }
        Err(e) => {
            println!("{:?}", e);
        }
    }
}

#[test]
fn extracting_from_file() {
    let tar = util::path::fixture("sample.tar.gz");
    let builder = reader::Builder::new().support_all().unwrap();
    let mut reader = builder.open_file(tar).ok().unwrap();
    println!("{:?}", reader.header_position());
    let writer = writer::Disk::new();
    let tempfile = tempfile::tempdir().unwrap();
    writer.write(&mut reader, tempfile.path().to_str()).unwrap();
    assert_fixture(&tempfile);
    println!("{:?}", reader.header_position());
    assert_eq!(4, 4)
}

#[test]
fn extracting_an_archive_with_options() {
    let tar = util::path::fixture("sample.tar.gz");
    let builder = reader::Builder::new().support_all().unwrap();
    let mut reader = builder.open_file(tar).ok().unwrap();
    println!("{:?}", reader.header_position());
    let mut opts = archive::ExtractOptions::new();
    opts.add(archive::ExtractOption::Time);
    let writer = writer::Disk::new();
    writer.set_options(&opts).ok();
    let tempfile = tempfile::tempdir().unwrap();
    writer.write(&mut reader, tempfile.path().to_str()).ok();
    assert_fixture(&tempfile);
    assert_eq!(reader.header_position(), 1024)
}

#[test]
fn extracting_a_reader_twice() {
    let tar = util::path::fixture("sample.tar.gz");
    let builder = reader::Builder::new().support_all().unwrap();
    let mut reader = builder.open_file(tar).ok().unwrap();
    println!("{:?}", reader.header_position());
    let tempfile = tempfile::tempdir().unwrap();
    let writer = writer::Disk::new();
    writer.write(&mut reader, tempfile.path().to_str()).ok();
    println!("{:?}", reader.header_position());
    writer
        .write(&mut reader, tempfile.path().to_str())
        .expect_err("writing twice should error");
    assert_fixture(&tempfile);
}

#[test]
fn read_in_memory() {
    let tar = util::path::fixture("sample.tar.gz");
    let reader = reader::Builder::new()
        .support_all()
        .unwrap()
        .open_file(tar)
        .unwrap();

    let mut iter = reader.into_iter();

    let mut hello = iter.next().unwrap().unwrap();
    assert_eq!(hello.pathname().unwrap().as_str(), "hello.txt");
    assert_eq!(hello.size(), 14);

    let mut string = String::new();
    hello.read_to_string(&mut string).unwrap();
    assert_string(&string);

    assert!(iter.next().is_none());
}

#[test]
fn panic_on_prev_iter() {
    let tar = util::path::fixture("sample.tar.gz");
    let reader = reader::Builder::new()
        .support_all()
        .unwrap()
        .open_file(tar)
        .unwrap();

    let mut iter = reader.into_iter();

    let hello = iter.next().unwrap().unwrap();

    assert!(iter.next().is_none());
    catch_unwind(AssertUnwindSafe(|| {
        hello.size();
    }))
    .expect_err("should panic");
}

fn reader() -> reader::ReaderHandle {
    reader::Builder::new()
        .support_all()
        .unwrap()
        .open_file(util::path::fixture("sample.tar.gz"))
        .unwrap()
}

#[test]
fn multiple_pathname_call() {
    let mut iter = reader().into_iter();
    let hello = iter.next().unwrap().unwrap();

    {
        assert_eq!(hello.pathname().unwrap().as_str(), "hello.txt");
    };
    {
        assert_eq!(hello.pathname().unwrap().as_str(), "hello.txt");
    };
}
