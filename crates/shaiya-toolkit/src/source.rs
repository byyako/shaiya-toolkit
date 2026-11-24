//! Where the previews read their files from. A preview is built from a path and a [`FileSource`], not from one kind of archive, so
//! the code that draws a model does not care where the bytes sit. The only implementation is the opened [`Archive`]; other client
//! layouts add theirs without any preview function changing its signature.

use shaiya_archive::Archive;
use shaiya_core::{Error, Job, Result, Unit};

/// A read-only set of files named by `/` paths, asked on a worker thread. Path lookups ignore letter case and take `/` or `\`.
pub trait FileSource: Send + Sync {
    /// Every readable file (a path that a later file of the same name hides is left out).
    fn paths(&self) -> Vec<String>;

    /// The size of the file in bytes, `None` when there is no such file.
    fn len(&self, path: &str) -> Option<u64>;

    /// The first `cap` bytes of the file (all of it when it is smaller). Progress is reported to `job` and a cancel ends the read.
    fn read(&self, path: &str, cap: u64, job: &Job) -> Result<Vec<u8>>;

    /// `n` bytes from `from` (fewer at the end of the file).
    fn read_range(&self, path: &str, from: u64, n: usize) -> Result<Vec<u8>>;

    /// Where the file sits and how long it is; what the caches of one source compare to know that a file is the one they learned
    /// about. `None` when there is no such file. For an archive this is the data offset and the length.
    fn stamp(&self, path: &str) -> Option<(u64, u64)>;

    /// Whether there is such a file.
    fn has(&self, path: &str) -> bool {
        self.len(path).is_some()
    }
}

/// A job that shares `job`'s cancel but reports no progress: the side reads of one preview (list files, textures, candidate meshes) use it,
/// so the progress shown is the main file's only.
pub fn quiet(job: &Job) -> Job<'static> {
    Job::new(&shaiya_core::Silent, job.cancel_token().clone())
}

fn not_found(path: &str) -> Error {
    Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, "not in the archive"))
}

impl FileSource for Archive {
    fn paths(&self) -> Vec<String> {
        self.index().visible().into_iter().map(|f| f.path).collect()
    }

    fn len(&self, path: &str) -> Option<u64> {
        self.index().get(path).map(|e| e.length as u64)
    }

    fn read(&self, path: &str, cap: u64, job: &Job) -> Result<Vec<u8>> {
        let entry = self.index().get(path).ok_or_else(|| not_found(path))?;
        let n = (entry.length as u64).min(cap) as usize;
        let mut out = Vec::with_capacity(n);
        job.total(Some(n as u64), Unit::Bytes);
        let mut done = 0usize;
        while done < n {
            job.check()?;
            let m = (n - done).min(1 << 20);
            out.extend_from_slice(&Archive::read_range(self, entry, done as u64, m)?);
            job.advance(m as u64);
            done += m;
        }
        Ok(out)
    }

    fn read_range(&self, path: &str, from: u64, n: usize) -> Result<Vec<u8>> {
        let entry = self.index().get(path).ok_or_else(|| not_found(path))?;
        Archive::read_range(self, entry, from, n)
    }

    fn stamp(&self, path: &str) -> Option<(u64, u64)> {
        self.index().get(path).map(|e| (e.offset, e.length as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::{SahBuilder, TempDir};

    #[test]
    fn the_archive_file_source_reads_like_the_archive() {
        let d = TempDir::new("w6-source");
        let big: Vec<u8> = (0..(3 << 20) + 5).map(|i| (i % 251) as u8).collect();
        SahBuilder::new(0).file(b"A/one.txt", b"hello").file(b"B/Two.bin", &big).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        let src: &dyn FileSource = &a;
        let mut paths = src.paths();
        paths.sort();
        let mut want: Vec<String> = a.index().visible().into_iter().map(|f| f.path).collect();
        want.sort();
        assert_eq!(paths, want);
        assert_eq!(paths.len(), 2);
        // length, in any letter case
        assert_eq!((src.len("A/one.txt"), src.len("a/ONE.txt"), src.len("A/none.txt")), (Some(5), Some(5), None));
        assert!(src.has("b\\two.bin") && !src.has("B/two.txt"));
        // whole reads equal the archive's, a cap cuts, a range equals the archive's range
        let job = Job::silent();
        assert_eq!(src.read("B/Two.bin", u64::MAX, &job).unwrap(), a.read("B/Two.bin").unwrap());
        assert_eq!(src.read("B/Two.bin", 1000, &job).unwrap(), big[..1000]);
        let e = a.index().get("B/Two.bin").unwrap();
        assert_eq!(src.read_range("B/Two.bin", 77, 4096).unwrap(), a.read_range(e, 77, 4096).unwrap());
        assert_eq!(src.stamp("B/Two.bin"), Some((e.offset, e.length as u64)));
        assert_eq!(src.stamp("B/none.bin"), None);
        // a file that is not there is an error, not an empty read
        assert!(src.read("B/none.bin", 10, &job).is_err() && src.read_range("B/none.bin", 0, 1).is_err());
    }

    #[test]
    fn a_cancelled_read_ends_cancelled() {
        let d = TempDir::new("w6-source-cancel");
        SahBuilder::new(0).file(b"A/one.txt", b"hello").write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        let cancel = shaiya_core::Cancel::new();
        cancel.cancel();
        let job = Job::new(&shaiya_core::Silent, cancel);
        assert!(matches!(FileSource::read(&a, "A/one.txt", 10, &job), Err(Error::Cancelled)));
    }
}
