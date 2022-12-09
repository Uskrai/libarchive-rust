use std::any::Any;
use std::cell::{Cell, RefCell};
use std::default::Default;
use std::ffi::CString;
use std::io::{self, Read};
use std::mem;
use std::path::Path;
use std::ptr;
use std::rc::Rc;
use std::slice;

use libarchive3_sys::ffi::{self};
use libc::{c_void, ssize_t};

use crate::archive::{Entry, Handle, ReadCompression, ReadFilter, ReadFormat};
use crate::error::{ArchiveError, ArchiveResult};

const BLOCK_SIZE: usize = 10240;

unsafe extern "C" fn stream_read_callback(
    handle: *mut ffi::Struct_archive,
    data: *mut c_void,
    buff: *mut *const c_void,
) -> ssize_t {
    let pipe: &mut Pipe = &mut *(data as *mut Pipe);
    *buff = pipe.buffer.as_mut_ptr() as *mut c_void;
    match pipe.read_bytes() {
        Ok(size) => size as ssize_t,
        Err(e) => {
            let desc = CString::new(e.to_string()).unwrap();
            ffi::archive_set_error(handle, e.raw_os_error().unwrap_or(0), desc.as_ptr());
            -1 as ssize_t
        }
    }
}

pub trait Reader: Handle + Sized {
    fn entry(&mut self) -> &mut ReaderEntryHandle;

    fn read_block(&self) -> ArchiveResult<Option<&[u8]>> {
        let mut buff = ptr::null();
        let mut size = 0;
        let mut offset = 0;

        unsafe {
            match ffi::archive_read_data_block(self.handle(), &mut buff, &mut size, &mut offset) {
                ffi::ARCHIVE_EOF => Ok(None),
                ffi::ARCHIVE_OK => Ok(Some(slice::from_raw_parts(buff as *const u8, size))),
                _ => Err(ArchiveError::Sys(self.err_code(), self.err_msg())),
            }
        }
    }
}

pub struct ArchiveIterator {
    reader: Rc<ReaderHandle>,
    entry: *mut ffi::Struct_archive_entry,
    current: std::rc::Rc<std::cell::Cell<Option<usize>>>,
}

impl Iterator for ArchiveIterator {
    type Item = ArchiveResult<ArchiveEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            let current = match self.current.get() {
                Some(v) => v + 1,
                None => 0,
            };
            self.current.set(Some(current));

            match ffi::archive_read_next_header(self.reader.handle, &mut self.entry) {
                ffi::ARCHIVE_OK => Some(Ok(ArchiveEntry::new(
                    self.reader.clone(),
                    self.entry,
                    self.current.clone(),
                    current,
                ))),
                ffi::ARCHIVE_EOF => None,
                _ => Some(Err(ArchiveError::from(self.reader.as_ref() as &dyn Handle))),
            }
        }
    }
}

pub struct ReaderHandle {
    handle: *mut ffi::Struct_archive,
    entry: ReaderEntryHandle,
    _pipe: Option<Box<Pipe>>,
}

impl Handle for ReaderHandle {
    unsafe fn handle(&self) -> *mut ffi::Struct_archive {
        self.handle
    }
}

impl ReaderHandle {
    fn new_file(handle: *mut ffi::Struct_archive) -> ReaderHandle {
        Self {
            handle,
            entry: Default::default(),
            _pipe: None,
        }
    }

    fn new_stream(handle: *mut ffi::Struct_archive, pipe: Box<Pipe>) -> ReaderHandle {
        Self {
            handle,
            entry: Default::default(),
            _pipe: Some(pipe),
        }
    }

    pub fn header_position(&self) -> i64 {
        unsafe { ffi::archive_read_header_position(self.handle) }
    }

    pub fn next_header(&mut self) -> Option<&mut ReaderEntryHandle> {
        let res = unsafe { ffi::archive_read_next_header(self.handle, &mut self.entry.handle) };
        if res == 0 {
            Some(&mut self.entry)
        } else {
            None
        }
    }
}

impl IntoIterator for ReaderHandle {
    type Item = ArchiveResult<ArchiveEntry>;

    type IntoIter = ArchiveIterator;

    fn into_iter(self) -> Self::IntoIter {
        ArchiveIterator {
            reader: Rc::new(self),
            entry: unsafe { ffi::archive_entry_new() },
            current: Default::default(),
        }
    }
}

impl Drop for ReaderHandle {
    fn drop(&mut self) {
        unsafe {
            ffi::archive_read_free(self.handle);
        }
    }
}

pub struct ArchiveEntry {
    handle: *mut ffi::Struct_archive_entry,
    reader: Rc<ReaderHandle>,
    iterator_current: std::rc::Rc<std::cell::Cell<Option<usize>>>,
    current: usize,

}

impl ArchiveEntry {
    pub fn new(
        reader: Rc<ReaderHandle>,
        handle: *mut ffi::Struct_archive_entry,
        iterator_current: Rc<Cell<Option<usize>>>,
        current: usize,
    ) -> Self {
        Self {
            handle,
            reader,
            iterator_current,
            current,
        }
    }
    pub fn is_current(&self) -> bool {
        self.iterator_current.get() == Some(self.current)
    }
    pub fn check_current(&self) {
        assert!(
            self.is_current(),
            "ArchiveEntry can only be used on current iterator item"
        );
    }

    pub fn pathname(&self) -> Option<String> {
        self.check_current();

        let pathname = unsafe { ffi::archive_entry_pathname(self.handle) };

        if pathname.is_null() {
            return None;
        }

        let pathname = unsafe { std::ffi::CStr::from_ptr(pathname) };

        let string = pathname.to_str().ok().map(|it| it.to_string())?;

        Some(string)
    }

    pub fn size(&self) -> i64 {
        self.check_current();
        unsafe { ffi::archive_entry_size(self.handle) }
    }

    pub fn filetype(&self) -> ArchiveEntryFiletype {
        self.check_current();
        let it = unsafe {
            match ffi::archive_entry_filetype(self.handle) {
                ffi::AE_IFREG => ArchiveEntryFiletype::RegularFile,
                ffi::AE_IFLNK => ArchiveEntryFiletype::SymbolicLink,
                ffi::AE_IFSOCK => ArchiveEntryFiletype::Socket,
                ffi::AE_IFCHR => ArchiveEntryFiletype::CharacterDevice,
                ffi::AE_IFDIR => ArchiveEntryFiletype::Directory,
                ffi::AE_IFIFO => ArchiveEntryFiletype::NamedPipe,
                i => {
                    ArchiveEntryFiletype::Unkown
                }
            }
        };
        it
    }

    pub fn is_directory(&self) -> bool {
        self.check_current();
        matches!(self.filetype(), ArchiveEntryFiletype::Directory)
    }

    pub fn is_file(&self) -> bool {
        self.check_current();
        matches!(self.filetype(), ArchiveEntryFiletype::RegularFile)
    }
}

impl Read for ArchiveEntry {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check_current();

        let size = unsafe {
            ffi::archive_read_data(self.reader.handle, buf.as_mut_ptr() as *mut c_void, buf.len())
        };

        if size < 0 {
            let err = ArchiveError::from(self as &dyn Handle);

            return Err(io::Error::new(io::ErrorKind::Other, err));
        }

        Ok(size.try_into().unwrap())
    }
}

impl Handle for ArchiveEntry {
    unsafe fn handle(&self) -> *mut ffi::Struct_archive {
        self.reader.handle
    }
}

#[derive(Debug)]
pub enum ArchiveEntryFiletype {
    RegularFile,
    SymbolicLink,
    Socket,
    CharacterDevice,
    Directory,
    NamedPipe,
    Unkown,
}

pub struct Builder {
    handle: *mut ffi::Struct_archive,
    consumed: bool,
}

pub struct ReaderEntryHandle {
    handle: *mut ffi::Struct_archive_entry,
}

struct Pipe {
    reader: Box<dyn Read>,
    buffer: Vec<u8>,
}

impl Pipe {
    fn new<T: Any + Read>(src: T) -> Self {
        Pipe {
            reader: Box::new(src),
            buffer: vec![0; 8192],
        }
    }

    fn read_bytes(&mut self) -> io::Result<usize> {
        self.reader.read(&mut self.buffer[..])
    }
}

impl Builder {
    pub fn new() -> Self {
        Builder::default()
    }

    pub fn support_compression(self, compression: ReadCompression) -> ArchiveResult<Self> {
        let result = match compression {
            ReadCompression::All => unsafe {
                ffi::archive_read_support_compression_all(self.handle)
            },
            ReadCompression::Bzip2 => unsafe {
                ffi::archive_read_support_compression_bzip2(self.handle)
            },
            ReadCompression::Compress => unsafe {
                ffi::archive_read_support_compression_compress(self.handle)
            },
            ReadCompression::Gzip => unsafe {
                ffi::archive_read_support_compression_gzip(self.handle)
            },
            ReadCompression::Lzip => unsafe {
                ffi::archive_read_support_compression_lzip(self.handle)
            },
            ReadCompression::Lzma => unsafe {
                ffi::archive_read_support_compression_lzma(self.handle)
            },
            ReadCompression::None => unsafe {
                ffi::archive_read_support_compression_none(self.handle)
            },
            ReadCompression::Program(prog) => {
                let c_prog = CString::new(prog).unwrap();
                unsafe {
                    ffi::archive_read_support_compression_program(self.handle, c_prog.as_ptr())
                }
            }
            ReadCompression::Rpm => unsafe {
                ffi::archive_read_support_compression_rpm(self.handle)
            },
            ReadCompression::Uu => unsafe { ffi::archive_read_support_compression_uu(self.handle) },
            ReadCompression::Xz => unsafe { ffi::archive_read_support_compression_xz(self.handle) },
        };
        match result {
            ffi::ARCHIVE_OK => Ok(self),
            _ => ArchiveResult::from(&self as &dyn Handle).map(|_| self),
        }
    }

    pub fn support_all(self) -> ArchiveResult<Self> {
        self.support_format(ReadFormat::All)?
            .support_filter(ReadFilter::All)?
            .support_compression(ReadCompression::All)
    }

    pub fn support_filter(self, filter: ReadFilter) -> ArchiveResult<Self> {
        let result = match filter {
            ReadFilter::All => unsafe { ffi::archive_read_support_filter_all(self.handle) },
            ReadFilter::Bzip2 => unsafe { ffi::archive_read_support_filter_bzip2(self.handle) },
            ReadFilter::Compress => unsafe {
                ffi::archive_read_support_filter_compress(self.handle)
            },
            ReadFilter::Grzip => unsafe { ffi::archive_read_support_filter_grzip(self.handle) },
            ReadFilter::Gzip => unsafe { ffi::archive_read_support_filter_gzip(self.handle) },
            ReadFilter::Lrzip => unsafe { ffi::archive_read_support_filter_lrzip(self.handle) },
            ReadFilter::Lzip => unsafe { ffi::archive_read_support_filter_lzip(self.handle) },
            ReadFilter::Lzma => unsafe { ffi::archive_read_support_filter_lzma(self.handle) },
            ReadFilter::Lzop => unsafe { ffi::archive_read_support_filter_lzop(self.handle) },
            ReadFilter::None => unsafe { ffi::archive_read_support_filter_none(self.handle) },
            ReadFilter::Program(prog) => {
                let c_prog = CString::new(prog).unwrap();
                unsafe { ffi::archive_read_support_filter_program(self.handle, c_prog.as_ptr()) }
            }
            ReadFilter::ProgramSignature(prog, cb, size) => {
                let c_prog = CString::new(prog).unwrap();
                unsafe {
                    ffi::archive_read_support_filter_program_signature(
                        self.handle,
                        c_prog.as_ptr(),
                        mem::transmute(cb),
                        size,
                    )
                }
            }
            ReadFilter::Rpm => unsafe { ffi::archive_read_support_filter_rpm(self.handle) },
            ReadFilter::Uu => unsafe { ffi::archive_read_support_filter_uu(self.handle) },
            ReadFilter::Xz => unsafe { ffi::archive_read_support_filter_xz(self.handle) },
        };
        match result {
            ffi::ARCHIVE_OK => Ok(self),
            _ => ArchiveResult::from(&self as &dyn Handle).map(|_| self),
        }
    }

    pub fn support_format(self, format: ReadFormat) -> ArchiveResult<Self> {
        let result = match format {
            ReadFormat::SevenZip => unsafe { ffi::archive_read_support_format_7zip(self.handle()) },
            ReadFormat::All => unsafe { ffi::archive_read_support_format_all(self.handle()) },
            ReadFormat::Ar => unsafe { ffi::archive_read_support_format_ar(self.handle()) },
            ReadFormat::Cab => unsafe { ffi::archive_read_support_format_cab(self.handle()) },
            ReadFormat::Cpio => unsafe { ffi::archive_read_support_format_cpio(self.handle()) },
            ReadFormat::Empty => unsafe { ffi::archive_read_support_format_empty(self.handle()) },
            ReadFormat::Gnutar => unsafe { ffi::archive_read_support_format_gnutar(self.handle()) },
            ReadFormat::Iso9660 => unsafe {
                ffi::archive_read_support_format_iso9660(self.handle())
            },
            ReadFormat::Lha => unsafe { ffi::archive_read_support_format_lha(self.handle()) },
            ReadFormat::Mtree => unsafe { ffi::archive_read_support_format_mtree(self.handle()) },
            ReadFormat::Rar => unsafe { ffi::archive_read_support_format_rar(self.handle()) },
            ReadFormat::Raw => unsafe { ffi::archive_read_support_format_raw(self.handle()) },
            ReadFormat::Tar => unsafe { ffi::archive_read_support_format_tar(self.handle()) },
            ReadFormat::Xar => unsafe { ffi::archive_read_support_format_xar(self.handle()) },
            ReadFormat::Zip => unsafe { ffi::archive_read_support_format_zip(self.handle()) },
        };
        match result {
            ffi::ARCHIVE_OK => Ok(self),
            _ => ArchiveResult::from(&self as &dyn Handle).map(|_| self),
        }
    }

    pub fn open_file<T: AsRef<Path>>(mut self, file: T) -> ArchiveResult<ReaderHandle> {
        self.check_consumed()?;

        let c_file = CString::new(file.as_ref().to_string_lossy().as_bytes()).unwrap();
        unsafe {
            match ffi::archive_read_open_filename(self.handle(), c_file.as_ptr(), BLOCK_SIZE) {
                ffi::ARCHIVE_OK => {
                    self.consume();
                    Ok(ReaderHandle::new_file(self.handle()))
                }
                _ => Err(ArchiveError::from(&self as &dyn Handle)),
            }
        }
        // FileReaderHandle::open(self, file)
    }

    pub fn open_stream<T: Any + Read>(mut self, src: T) -> ArchiveResult<ReaderHandle> {
        self.check_consumed()?;

        unsafe {
            let mut pipe = Box::new(Pipe::new(src));
            let pipe_ptr: *mut c_void = &mut *pipe as *mut Pipe as *mut c_void;
            match ffi::archive_read_open(
                self.handle(),
                pipe_ptr,
                None,
                Some(stream_read_callback),
                None,
            ) {
                ffi::ARCHIVE_OK => {
                    self.consume();
                    Ok(ReaderHandle::new_stream(self.handle(), pipe))
                }
                _ => {
                    self.consume();
                    Err(ArchiveError::from(&self as &dyn Handle))
                }
            }
        }
    }

    fn check_consumed(&self) -> ArchiveResult<()> {
        if self.consumed {
            Err(ArchiveError::Consumed)
        } else {
            Ok(())
        }
    }

    fn consume(&mut self) {
        self.consumed = true;
    }
}

impl Handle for Builder {
    unsafe fn handle(&self) -> *mut ffi::Struct_archive {
        self.handle
    }
}

impl Drop for Builder {
    fn drop(&mut self) {
        if !self.consumed {
            unsafe {
                ffi::archive_read_free(self.handle);
            }
        }
    }
}

impl Default for Builder {
    fn default() -> Self {
        unsafe {
            let handle = ffi::archive_read_new();
            if handle.is_null() {
                panic!("Allocation error");
            }
            Builder {
                handle,
                consumed: false,
            }
        }
    }
}

impl ReaderEntryHandle {
    pub fn new(handle: *mut ffi::Struct_archive_entry) -> Self {
        ReaderEntryHandle { handle }
    }
}

impl Default for ReaderEntryHandle {
    fn default() -> Self {
        ReaderEntryHandle {
            handle: ptr::null_mut(),
        }
    }
}

impl Entry for ReaderEntryHandle {
    unsafe fn entry(&self) -> *mut ffi::Struct_archive_entry {
        self.handle
    }
}
