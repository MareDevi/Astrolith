//! Bounded readonly source loading. This module never writes a document.
use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
};

pub(crate) struct DocumentSource {
    pub path: PathBuf,
    pub source: String,
}

pub(crate) fn read_document(path: PathBuf) -> io::Result<DocumentSource> {
    let file = File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other(tr!("services.file.regular_file_only")));
    }
    let mut bytes = Vec::new();
    file.take((astrolith_document::MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > astrolith_document::MAX_DOCUMENT_BYTES {
        return Err(io::Error::other(tr!("services.file.too_large")));
    }
    let source =
        String::from_utf8(bytes).map_err(|_| io::Error::other(tr!("services.file.not_utf8")))?;
    if source.lines().count() > 50000 {
        return Err(io::Error::other(tr!("services.file.too_many_lines")));
    }
    Ok(DocumentSource { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_and_mdx_snapshots_preserve_source_bytes() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["中文.md", "page.mdx"] {
            let path = directory.path().join(name);
            let source =
                "---\r\ntitle: 日本語\r\n---\r\nimport X from './X';\r\n<X>{1 + 1}</X> 🦀\r\n";
            std::fs::write(&path, source).unwrap();
            let snapshot = read_document(path.clone()).unwrap();
            assert_eq!(snapshot.source, source);
            assert_eq!(snapshot.path, path);
            assert_eq!(std::fs::read_to_string(&snapshot.path).unwrap(), source);
        }
    }

    #[test]
    fn invalid_utf8_missing_nonregular_and_oversized_sources_fail_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        assert!(read_document(directory.path().join("missing.md")).is_err());
        assert!(read_document(directory.path().to_path_buf()).is_err());
        let path = directory.path().join("invalid.md");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(read_document(path.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe]);
        let path = directory.path().join("large.md");
        std::fs::File::create(&path)
            .unwrap()
            .set_len((astrolith_document::MAX_DOCUMENT_BYTES + 1) as u64)
            .unwrap();
        assert!(read_document(path).is_err());
    }
}
