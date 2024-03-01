use id3::TagLike;

pub trait Music {
    fn path(&self) -> &str;
    fn bpm(&self) -> Option<u32>;
    fn set_bpm(&mut self, bpm: u32) -> Result<(), anyhow::Error>;
}

pub struct Mp3 {
    path: String,
    bpm: Option<u32>,
}

impl Mp3 {
    pub fn new(path: String) -> Result<Mp3, anyhow::Error> {
        let tag = match id3::Tag::read_from_path(&path) {
            Ok(tag) => Some(tag),
            Err(id3::Error {
                kind: id3::ErrorKind::NoTag,
                ..
            }) => None,
            Err(e) => return Err(e.into()),
        };

        let bpm = tag
            .as_ref()
            .and_then(|tag| tag.get("TBPM"))
            .and_then(|bpm| bpm.content().text())
            .and_then(|bpm| bpm.parse().ok());

        Ok(Mp3 { path, bpm })
    }
}

impl Music for Mp3 {
    fn path(&self) -> &str {
        &self.path
    }

    fn bpm(&self) -> Option<u32> {
        self.bpm
    }

    fn set_bpm(&mut self, bpm: u32) -> Result<(), anyhow::Error> {
        self.bpm = Some(bpm);
        let mut tag = id3::Tag::read_from_path(&self.path).map_err(Into::<anyhow::Error>::into)?;
        tag.set_text("TBPM", bpm.to_string());
        tag.write_to_path(&self.path, id3::Version::Id3v24)
            .map_err(Into::<anyhow::Error>::into)?;

        Ok(())
    }
}

pub struct Flac {
    path: String,
    bpm: Option<u32>,
}

impl Flac {
    pub fn new(path: String) -> Result<Flac, anyhow::Error> {
        let tag = metaflac::Tag::read_from_path(&path)?;
        let bpm = tag
            .get_vorbis("BPM")
            .and_then(|mut bpm| bpm.next())
            .and_then(|bpm| bpm.parse().ok());

        Ok(Flac { path, bpm })
    }
}

impl Music for Flac {
    fn path(&self) -> &str {
        &self.path
    }

    fn bpm(&self) -> Option<u32> {
        self.bpm
    }

    fn set_bpm(&mut self, bpm: u32) -> Result<(), anyhow::Error> {
        self.bpm = Some(bpm);
        let mut tag =
            metaflac::Tag::read_from_path(&self.path).map_err(Into::<anyhow::Error>::into)?;
        tag.set_vorbis("BPM", vec![bpm.to_string()]);
        tag.save().map_err(Into::<anyhow::Error>::into)?;

        Ok(())
    }
}
