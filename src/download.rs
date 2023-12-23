use crate::peer::Peer;
use crate::piece::Piece;
use crate::torrent::{File, Keys, Torrent};
use crate::tracker::TrackerResponse;
use crate::BLOCK_MAX;
use anyhow::Context;
use sha1::{Digest, Sha1};
use std::collections::BinaryHeap;
use std::net::SocketAddrV4;
use futures_util::StreamExt;

pub(crate) async fn all(t: &Torrent) -> anyhow::Result<Downloaded> {
    let info_hash = t.info_hash();
    let peer_info = TrackerResponse::query(t, &info_hash)
        .await
        .context("query tracker for peer info")?;

    let mut peer_list = Vec::new();
    let mut peers = futures_util::stream::iter(peer_info.peers.0.iter())
        .map(|&peer_addr| async move {
            let peer = Peer::new(peer_addr, info_hash).await;
            (peer_addr, peer)
        })
        .buffer_unordered(5 /* user config */);

    while let Some((peer_addr, peer)) = peers.next().await {
        match peer {
            Ok(peer) => {
                peer_list.push(peer);
                if peer_list.len() >= 5 {
                    /* TODO user config */
                    break;
                }
            }
            Err(e) => eprintln!("failed co connect to peer {peer_addr:?} {e}"),
        }
    }
    drop(peers);

    // you pick pieces with fewer number of peers first, we use BinaryHeap for that
    let mut peers = peer_list;

    let mut need_pieces = BinaryHeap::new();
    let mut no_peers = Vec::new();
    for piece_i in 0..t.info.pieces.0.len() {
        let piece = Piece::new(piece_i, &t, &peers);
        if piece.peers().is_empty() {
            no_peers.push(piece)
        } else {
            need_pieces.push(piece)
        }
    }

    // TODO this is dumb
    let mut all_pieces = vec![0; t.length()];
    while let Some(piece) = need_pieces.pop() {
        // the + (BLOCK_MAX - 1) rounds up
        let piece_size = piece.length();
        let nblocks = (piece_size + (BLOCK_MAX - 1)) / BLOCK_MAX;
        //let mut all_blocks = Vec::with_capacity(piece_size);
        let peers: Vec<_> = peers
            .iter_mut()
            .enumerate()
            .filter_map(|(peer_i, peer)| piece.peers().contains(&peer_i).then_some(peer))
            .collect();

        let (submit, tasks) = kanal::bounded_async(nblocks);
        for block in 0..nblocks {
            submit
                .send(block)
                .await
                .expect("bound holds all these items");
        }

        let (finish, mut done) = tokio::sync::mpsc::channel(nblocks);
        let mut participants = futures_util::stream::futures_unordered::FuturesUnordered::new();

        for peer in peers {
            participants.push(peer.participate(
                piece.index(),
                piece_size,
                nblocks,
                submit.clone(),
                tasks.clone(),
                finish.clone(),
            ));
        }

        drop(submit);
        drop(finish);
        drop(tasks);

        let mut all_blocks = vec![0u8; piece_size];
        let mut bytes_received = 0;
        loop {
            tokio::select! {
                joined = participants.next(), if !participants.is_empty() => {
                    // if a participant ends early, it is either slow or failed
                    match joined {
                        None => {
                            // there are no peers
                            // this must mean we are about to get None from done.recv()
                            // so we will handle it there
                        },
                        Some(Ok(_)) => {
                            // the peer gave up beacause it timed out
                        },
                        Some(Err(_)) => {
                            // peer failed and should be removed later
                            // we should not try this peer again
                            // and should remove it from a global peer list
                            // TODO
                        },

                    }

                }
                piece = done.recv() => {
                    if let Some(piece) = piece {
                        // keep track of the bytes in message
                        let piece = crate::peer::Piece::ref_from_bytes(&piece.payload[..])
                        .expect("always get all Piece response fields from peer");
                        // assert_eq!(piece.begin() as usize, block * BLOCK_MAX);
                        // assert_eq!(piece.block().len(), block_size);
                        all_blocks[piece.begin() as usize..].copy_from_slice(piece.block());
                        bytes_received += piece.block().len();
                    } else {
                        // have received every piece or no peers left

                        // this meanns that all participations are either exited or waiting for more work
                        //in either case it is okay to drop all participant future
                        break;
                    }

                }
            }
        }

        drop(participants);
        if bytes_received == piece_size {
            // great, we got all the bytes
        } else {
            // all the peers quit on us
            // we ll need to conenct to additional peers and make sure these peers have this piece
            // and download the pieces we did not get
            anyhow::bail!("no peers left to get piece {}", piece.index())
        }

        let mut hasher = Sha1::new();
        hasher.update(&all_blocks);
        let hash: [u8;20]= hasher
            .finalize()
            .try_into()
            .expect("GenericArray<_, 20> == [_, 20]");
        assert_eq!(hash, piece.hash());

        all_pieces[piece.index() * t.info.plength..][..piece_size].copy_from_slice(&all_blocks);
    }

    Ok(Downloaded {
        bytes: all_pieces,
        files: match &t.info.keys {
            Keys::SingleFile { length } => vec![File {
                length: *length,
                path: vec![t.info.name.clone()],
            }],
            Keys::MultiFile { files } => files.clone(),
        },
    })
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
