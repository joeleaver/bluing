use bluing_base::FileId;
use bluing_syntax::ra_ap_syntax::ast;

use crate::VfsPath;

/// Every source file loaded during a compilation.
#[derive(Default)]
pub struct Files {
    files: Vec<FileData>,
}

pub struct FileData {
    pub path: VfsPath,
    pub text: String,
    pub tree: ast::SourceFile,
}

impl Files {
    /// The id the next added file will get.
    pub(crate) fn next_id(&self) -> FileId {
        FileId(u32::try_from(self.files.len()).expect("more than u32::MAX files"))
    }

    pub(crate) fn add(&mut self, data: FileData) -> FileId {
        let id = self.next_id();
        self.files.push(data);
        id
    }

    pub fn get(&self, id: FileId) -> &FileData {
        &self.files[id.0 as usize]
    }

    pub fn iter(&self) -> impl Iterator<Item = (FileId, &FileData)> {
        (0u32..).map(FileId).zip(&self.files)
    }
}
