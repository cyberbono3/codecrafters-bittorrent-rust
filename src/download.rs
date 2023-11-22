use crate::torrent::{File, Torrent};
use crate::tracker::TrackerResponse;
use std::net::SocketAddrV4;

pub(crate) async fn all(t: &Torrent) -> anyhow::Result<Downloaded> {
    let peer_info = TrackerResponse::query(t)
        .await
        .context("query tracker for peer info")?;
}

pub async fn download_piece(
    candidate_peers: &[SocketAddrV4],
    piece_hash: [u8; 20],
    piece_size: usize,
) {
}

pub async fn download_piece_block_from(peer: &SocketAddrV4, block_i: usize, block_size: usize) {}

pub struct Downloaded {
    bytes: Vec<u8>, // TODO: maybe Bytes
    files: Vec<File>,
}

impl<'a> IntoIterator for &'a Downloaded {
    type Item = DownloadedFile<'a>;
    type IntoIter = DownloadedIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        DownloadedIter::new(self)
    }
}

pub struct DownloadedIter<'d> {
    downloaded: &'d Downloaded,
    file_iter: std::slice::Iter<'d, File>,
    offset: usize, // denotes how far we are through bytes
}

impl<'d> DownloadedIter<'d> {
    fn new(d: &'d Downloaded) -> Self {
        Self {
            downloaded: d,
            file_iter: d.files.iter(),
            offset: 0,
        }
    }
}

impl<'d> Iterator for DownloadedIter<'d> {
    type Item = DownloadedFile<'d>;

    fn next(&mut self) -> Option<Self::Item> {
        let file = self.file_iter.next()?;
        let bytes = &self.downloaded.bytes[self.offset..][..file.length]; // i dont need to repeat self.offset second time
        Some(DownloadedFile { file, bytes })
    }
}

pub struct DownloadedFile<'d> {
    file: &'d File,
    bytes: &'d [u8],
}

impl<'d> DownloadedFile<'d> {
    pub fn path(&self) -> &'d [String] {
        &self.file.path
    }
    pub fn bytes(&self) -> &'d [u8] {
        self.bytes
    }
}
