use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::error::LoggerResult;
use crate::sample::Sample;

#[derive(Debug)]
pub struct DatalogWriter {
    path: PathBuf,
    out: BufWriter<File>,
    header_written: bool,
}

impl DatalogWriter {
    pub fn create(path: impl AsRef<Path>) -> LoggerResult<Self> {
        let file = File::create(path.as_ref())?;
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            out: BufWriter::new(file),
            header_written: false,
        })
    }

    pub fn write_sample(&mut self, sample: &Sample) -> LoggerResult<()> {
        if !self.header_written {
            self.write_header(sample)?;
        }
        let ts = sample
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        write!(self.out, "{ts}")?;
        for v in &sample.values {
            write!(self.out, ",{}", v.value)?;
        }
        writeln!(self.out)?;
        Ok(())
    }

    fn write_header(&mut self, sample: &Sample) -> LoggerResult<()> {
        write!(self.out, "timestamp_ms")?;
        for v in &sample.values {
            write!(self.out, ",{}", v.parameter_id)?;
        }
        writeln!(self.out)?;
        self.header_written = true;
        Ok(())
    }

    pub fn flush(&mut self) -> LoggerResult<()> {
        self.out.flush()?;
        Ok(())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
