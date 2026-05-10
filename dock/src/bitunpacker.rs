use std::{
    fs,
    io::{self, Read},
    path::Path,
    sync::Arc,
    usize,
};

#[derive(thiserror::Error, Debug)]
pub enum ZippedBMaskCreateError {
    #[error("archive open: {0}")]
    ArchiveOpen(io::Error),

    #[error("byte read: {0}")]
    ByteRead(io::Error),

    #[error("zip error: {0}")]
    ZipArchiveNew(#[from] zip::result::ZipError),
}

#[derive(Clone)]
pub struct BMaskFrame(Arc<[u8]>);

impl std::ops::Deref for BMaskFrame {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct BMask {
    frames: Vec<BMaskFrame>,
}

impl BMask {
    pub fn new<P>(path: P) -> Result<Self, ZippedBMaskCreateError>
    where
        P: AsRef<Path>,
    {
        let file = fs::File::open(path).map_err(ZippedBMaskCreateError::ArchiveOpen)?;
        let mut archive = zip::read::ZipArchive::new(file)?;

        let mut file_names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).map(|f| f.name().to_string()))
            .collect::<Result<_, _>>()?;
        file_names.sort();

        let mut frames = Vec::<BMaskFrame>::with_capacity(file_names.len());
        for name in &file_names {
            let mut file = archive.by_name(name)?;
            let mut buffer = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buffer)
                .map_err(ZippedBMaskCreateError::ByteRead)?;
            frames.push(BMaskFrame(Arc::from(buffer)));
        }

        Ok(BMask { frames })
    }

    pub fn sample(&self, frame: u64) -> BMaskFrame {
        let frame_count = self.frames.len();
        let frame = (frame % frame_count as u64) as usize;
        self.frames[frame].clone()
    }
}
