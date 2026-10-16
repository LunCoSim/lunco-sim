//! Origin Private File System (OPFS) backend for wasm — the web counterpart to
//! [`crate::FileStorage`] for **binary assets** (meshes, textures, DEMs), where
//! [`crate::WebStorage`]'s `localStorage`+hex (2× size, string-only) is unusable.
//!
//! # Why inherent methods, not `impl Storage`
//!
//! The [`Storage`](crate::Storage) trait requires `Send` futures (it's
//! `#[async_trait]` over `Send + Sync`). OPFS calls hold non-`Send` JS values
//! (`Promise`, `JsValue`, the handle types) **across `.await`**, so an OPFS
//! future can't be `Send`. Rather than fork the trait, `OpfsStorage` exposes
//! **inherent** `async fn`s with the same shape; callers drive them with
//! `wasm_bindgen_futures::spawn_local` (which, unlike a thread pool, does not
//! require `Send`). Native code keeps using the `Send` [`Storage`] trait +
//! `AsyncComputeTaskPool`. The shared caller (`lunco-networking`'s asset sync)
//! therefore diverges only at a one-line `#[cfg]` backend pick.
//!
//! # Handle mapping
//!
//! Deliberately reuses [`StorageHandle::File`] rather than adding an `Opfs`
//! variant — the path's `Normal` components become the OPFS directory/file tree
//! (`getDirectoryHandle`/`getFileHandle` with `{create:true}`), so the same
//! `File(path)` handle addresses the native FS on desktop and OPFS on web with no
//! enum change (and no exhaustiveness churn across the workspace). A relative
//! path (`scenarios/<id>/rover.glb`) is the intended input on web; absolute /
//! root markers select the same private tree; parent traversal, native prefixes,
//! and invalid names are rejected by the shared canonical identity boundary.
//!
//! # Non-blocking
//!
//! Writes go through the **async** `createWritable()` stream — legal on the main
//! thread and non-blocking (the browser streams to disk off-thread). The
//! worker-only synchronous `FileSystemSyncAccessHandle` is intentionally NOT
//! used, so no Web Worker is required.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::{StorageError, StorageHandle, StorageResult};

/// Demand-driven OPFS bytes. At most one 8 KiB slice is in flight, so an
/// `AsyncRead::take` consumer controls allocation before payload materialization.
pub struct OpfsReader {
    requests: async_channel::Sender<(u64, usize)>,
    responses: std::pin::Pin<Box<async_channel::Receiver<std::io::Result<Vec<u8>>>>>,
    pending: Option<u64>,
    buffered: Vec<u8>,
    buffer_start: u64,
    position: u64,
    length: u64,
}

impl futures_lite::io::AsyncRead for OpfsReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use futures_lite::Stream;
        use std::task::Poll;
        let this = self.get_mut();
        loop {
            if buffer.is_empty() || this.position >= this.length {
                return Poll::Ready(Ok(0));
            }
            if let Some(index) = this
                .position
                .checked_sub(this.buffer_start)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < this.buffered.len())
            {
                let count = buffer.len().min(this.buffered.len() - index);
                buffer[..count].copy_from_slice(&this.buffered[index..index + count]);
                this.position += count as u64;
                return Poll::Ready(Ok(count));
            }
            if this.pending.is_none() {
                if this
                    .requests
                    .try_send((this.position, buffer.len().min(8192)))
                    .is_err()
                {
                    return Poll::Ready(Err(std::io::Error::other("OPFS reader is closed")));
                }
                this.pending = Some(this.position);
            }
            match this.responses.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(result)) => {
                    // A cancelled read may have been followed by a seek. Its
                    // response retains the original offset and cannot move the
                    // current cursor or masquerade as bytes from the new one.
                    this.buffer_start = this
                        .pending
                        .take()
                        .expect("response has an outstanding request");
                    match result {
                        Ok(bytes) if !bytes.is_empty() => this.buffered = bytes,
                        Ok(_) => {
                            return Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "OPFS snapshot ended before its declared size",
                            )));
                        }
                        Err(error) => return Poll::Ready(Err(error)),
                    }
                }
                Poll::Ready(None) => {
                    return Poll::Ready(Err(std::io::Error::other("OPFS reader task ended")));
                }
            }
        }
    }
}

impl futures_lite::io::AsyncSeek for OpfsReader {
    fn poll_seek(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        from: std::io::SeekFrom,
    ) -> std::task::Poll<std::io::Result<u64>> {
        let this = self.get_mut();
        let position = match from {
            std::io::SeekFrom::Start(position) => i128::from(position),
            std::io::SeekFrom::Current(offset) => i128::from(this.position) + i128::from(offset),
            std::io::SeekFrom::End(offset) => i128::from(this.length) + i128::from(offset),
        };
        std::task::Poll::Ready(
            u64::try_from(position)
                .map(|position| {
                    this.position = position;
                    position
                })
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "invalid OPFS seek position",
                    )
                }),
        )
    }
}

/// JavaScript File sizes and Blob slice offsets are exact only through 2^53-1.
fn snapshot_len(file: &web_sys::File) -> StorageResult<u64> {
    let size = file.size();
    if !size.is_finite() || size < 0.0 || size.fract() != 0.0 || size > 9_007_199_254_740_991.0 {
        return Err(StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "OPFS returned an invalid File size",
        )));
    }
    Ok(size as u64)
}

fn js_io_error(error: JsValue) -> std::io::Error {
    std::io::Error::other(format!("opfs: {error:?}"))
}

/// OPFS-backed storage for wasm binary assets. Zero-sized; all state lives in
/// the origin's private file system, shared across every instance.
#[derive(Default)]
pub struct OpfsStorage;

impl OpfsStorage {
    /// Construct a fresh backend handle.
    pub fn new() -> Self {
        Self
    }

    /// Write `bytes` to the OPFS path addressed by `handle` (a
    /// [`StorageHandle::File`]), creating intermediate directories. Async +
    /// non-blocking (`createWritable`). See the module docs for the handle
    /// mapping and the "inherent, not `impl Storage`" rationale.
    pub async fn write(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        let (dirs, file) = split_handle(handle)?;
        let dir = resolve_dir(&segments_root().await?, &dirs, true).await?;
        let file_handle = get_file(&dir, &file, true).await?;
        let writable = JsFuture::from(file_handle.create_writable())
            .await
            .map_err(js_err)?
            .dyn_into::<web_sys::FileSystemWritableFileStream>()
            .map_err(|_| unsupported("createWritable did not return a writable stream"))?;
        // `write_with_u8_array` copies the bytes into a browser-owned buffer; the
        // actual disk write is streamed asynchronously by the UA.
        let write_promise = writable.write_with_u8_array(bytes).map_err(js_err)?;
        JsFuture::from(write_promise).await.map_err(js_err)?;
        JsFuture::from(writable.close()).await.map_err(js_err)?;
        Ok(())
    }

    /// Read the full contents of the OPFS file addressed by `handle`.
    pub async fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
        self.read_contents(handle, None).await
    }

    /// Read within the caller's byte budget. The immutable File snapshot's
    /// size is checked before allocating its array buffer or Rust byte vector.
    pub async fn read_bounded(
        &self,
        handle: &StorageHandle,
        max_bytes: usize,
    ) -> StorageResult<Vec<u8>> {
        self.read_contents(handle, Some(max_bytes)).await
    }

    async fn file_snapshot(&self, handle: &StorageHandle) -> StorageResult<web_sys::File> {
        let (dirs, file) = split_handle(handle)?;
        let dir = resolve_dir(&segments_root().await?, &dirs, false).await?;
        let file_handle = get_file(&dir, &file, false).await?;
        JsFuture::from(file_handle.get_file())
            .await
            .map_err(|_| StorageError::NotFound)?
            .dyn_into::<web_sys::File>()
            .map_err(|_| unsupported("getFile did not return a File"))
    }

    /// Open an immutable browser File snapshot without reading its bytes.
    /// A local task owns JavaScript values; the movable reader exchanges only
    /// bounded Rust chunks. Dropping it retires the task through channel close.
    pub async fn open_reader(&self, handle: &StorageHandle) -> StorageResult<OpfsReader> {
        let file = self.file_snapshot(handle).await?;
        let length = snapshot_len(&file)?;
        let (request_tx, request_rx) = async_channel::bounded::<(u64, usize)>(1);
        let (response_tx, response_rx) = async_channel::bounded(1);
        wasm_bindgen_futures::spawn_local(async move {
            while let Ok((offset, count)) = request_rx.recv().await {
                let result = async {
                    let offset = offset as f64;
                    let end = offset
                        + f64::from(u32::try_from(count).map_err(|_| {
                            std::io::Error::other("OPFS chunk exceeds the wasm32 address space")
                        })?);
                    let slice = file
                        .slice_with_f64_and_f64(offset, end)
                        .map_err(js_io_error)?;
                    let buffer = JsFuture::from(slice.array_buffer())
                        .await
                        .map_err(js_io_error)?;
                    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                    Ok(bytes)
                }
                .await;
                let failed = result.is_err();
                if response_tx.send(result).await.is_err() || failed {
                    break;
                }
            }
        });
        Ok(OpfsReader {
            requests: request_tx,
            responses: Box::pin(response_rx),
            pending: None,
            buffered: Vec::new(),
            buffer_start: 0,
            position: 0,
            length,
        })
    }

    async fn read_contents(
        &self,
        handle: &StorageHandle,
        max_bytes: Option<usize>,
    ) -> StorageResult<Vec<u8>> {
        let file = self.file_snapshot(handle).await?;
        if let Some(max_bytes) = max_bytes {
            let size = snapshot_len(&file)?;
            // wasm32 budgets are exact in JavaScript's numeric representation.
            let limit = u64::try_from(max_bytes)
                .map_err(|_| unsupported("read budget exceeds the address space"))?;
            if size > limit {
                return Err(StorageError::SizeLimitExceeded { max_bytes });
            }
        }
        let buf = JsFuture::from(file.array_buffer()).await.map_err(js_err)?;
        let array = js_sys::Uint8Array::new(&buf);
        Ok(array.to_vec())
    }

    /// List direct children with the same File-handle mapping as native storage.
    pub async fn read_directory(
        &self,
        handle: &StorageHandle,
    ) -> StorageResult<Vec<StorageHandle>> {
        self.read_directory_contents(handle, None)
            .await
            .map(|snapshot| snapshot.entries)
    }
    /// Stream regular direct files, retaining at most the lexical cap.
    pub async fn read_directory_bounded(
        &self,
        handle: &StorageHandle,
        cap: usize,
    ) -> StorageResult<crate::BoundedDirectoryEntries> {
        self.read_directory_contents(handle, Some(cap)).await
    }
    async fn read_directory_contents(
        &self,
        handle: &StorageHandle,
        cap: Option<usize>,
    ) -> StorageResult<crate::BoundedDirectoryEntries> {
        let path = handle
            .as_file_path()
            .ok_or_else(|| unsupported("OpfsStorage addresses File handles only"))?;
        let path = crate::canonicalize_file_path(path)?;
        let segments = path
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let directory = resolve_dir(&segments_root().await?, &segments, false).await?;
        let iterator = directory.entries();
        let mut snapshot = crate::BoundedDirectoryEntries {
            entries: Vec::new(),
            truncated: false,
        };
        loop {
            let result = JsFuture::from(iterator.next().map_err(js_err)?)
                .await
                .map_err(js_err)?;
            let done = js_sys::Reflect::get(&result, &JsValue::from_str("done"))
                .map_err(js_err)?
                .as_bool()
                .ok_or_else(|| unsupported("OPFS directory iterator returned no done flag"))?;
            if done {
                break;
            }
            let entry = js_sys::Array::from(
                &js_sys::Reflect::get(&result, &JsValue::from_str("value")).map_err(js_err)?,
            );
            let name = entry
                .get(0)
                .as_string()
                .ok_or_else(|| unsupported("OPFS directory iterator returned no name"))?;
            if cap.is_some() {
                let handle = entry.get(1);
                if handle.is_instance_of::<web_sys::FileSystemDirectoryHandle>() {
                    continue;
                }
                handle
                    .dyn_into::<web_sys::FileSystemFileHandle>()
                    .map_err(|_| {
                        unsupported("OPFS directory iterator returned an unknown entry kind")
                    })?;
            }
            let entry = StorageHandle::File(path.join(name));
            if let Some(cap) = cap {
                snapshot.retain_entry(entry, cap);
            } else {
                snapshot.entries.push(entry);
            }
        }
        if cap.is_none() {
            snapshot.entries.sort_by_key(|entry| entry.display_name());
        }
        Ok(snapshot)
    }

    /// Delete the OPFS file addressed by `handle`. [`StorageError::NotFound`]
    /// when the file (or any directory on its path) doesn't exist.
    pub async fn delete(&self, handle: &StorageHandle) -> StorageResult<()> {
        let (dirs, file) = split_handle(handle)?;
        let dir = resolve_dir(&segments_root().await?, &dirs, false).await?;
        JsFuture::from(dir.remove_entry(&file))
            .await
            .map_err(|_| StorageError::NotFound)?;
        Ok(())
    }

    /// Whether the OPFS file addressed by `handle` exists.
    pub async fn exists(&self, handle: &StorageHandle) -> bool {
        let Ok((dirs, file)) = split_handle(handle) else {
            return false;
        };
        let Ok(root) = segments_root().await else {
            return false;
        };
        let Ok(dir) = resolve_dir(&root, &dirs, false).await else {
            return false;
        };
        get_file(&dir, &file, false).await.is_ok()
    }
}

/// The OPFS root directory handle (`navigator.storage.getDirectory()`).
async fn segments_root() -> StorageResult<web_sys::FileSystemDirectoryHandle> {
    let navigator = web_sys::window()
        .map(|w| w.navigator())
        .ok_or_else(|| unsupported("no window (OPFS needs a browsing context)"))?;
    let manager = navigator.storage();
    JsFuture::from(manager.get_directory())
        .await
        .map_err(js_err)?
        .dyn_into::<web_sys::FileSystemDirectoryHandle>()
        .map_err(|_| unsupported("getDirectory did not return a directory handle"))
}

/// Split a `File` handle into `(intermediate dir names, final file name)`,
/// using the same validated identity as mounted-file admission.
fn split_handle(handle: &StorageHandle) -> StorageResult<(Vec<String>, String)> {
    let path = handle
        .as_file_path()
        .ok_or_else(|| unsupported("OpfsStorage addresses File handles only"))?;
    let canonical = crate::canonicalize_file_path(path)?;
    let mut comps: Vec<String> = canonical
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect();
    let file = comps.pop().ok_or(StorageError::NotFound)?;
    Ok((comps, file))
}

/// Walk (optionally creating) `segments` under `root`, returning the final dir.
async fn resolve_dir(
    root: &web_sys::FileSystemDirectoryHandle,
    segments: &[String],
    create: bool,
) -> StorageResult<web_sys::FileSystemDirectoryHandle> {
    let mut dir = root.clone();
    for seg in segments {
        let opts = get_options(create);
        let next = JsFuture::from(dir.get_directory_handle_with_options(seg, opts.unchecked_ref()))
            .await
            .map_err(|e| {
                if create {
                    js_err(e)
                } else {
                    StorageError::NotFound
                }
            })?
            .dyn_into::<web_sys::FileSystemDirectoryHandle>()
            .map_err(|_| unsupported("getDirectoryHandle returned a non-directory"))?;
        dir = next;
    }
    Ok(dir)
}

/// Get (optionally creating) a file handle named `name` under `dir`.
async fn get_file(
    dir: &web_sys::FileSystemDirectoryHandle,
    name: &str,
    create: bool,
) -> StorageResult<web_sys::FileSystemFileHandle> {
    let opts = get_options(create);
    JsFuture::from(dir.get_file_handle_with_options(name, opts.unchecked_ref()))
        .await
        .map_err(|e| {
            if create {
                js_err(e)
            } else {
                StorageError::NotFound
            }
        })?
        .dyn_into::<web_sys::FileSystemFileHandle>()
        .map_err(|_| unsupported("getFileHandle returned a non-file"))
}

/// A `{create: bool}` options object, built via `Reflect` so it's independent of
/// web-sys's per-version option-setter API.
fn get_options(create: bool) -> JsValue {
    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &o,
        &JsValue::from_str("create"),
        &JsValue::from_bool(create),
    );
    o.into()
}

fn js_err(e: JsValue) -> StorageError {
    StorageError::Io(std::io::Error::other(format!("opfs: {e:?}")))
}

fn unsupported(msg: &str) -> StorageError {
    StorageError::Unsupported(msg.to_string())
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use futures_lite::io::{AsyncReadExt, AsyncSeekExt};

    #[wasm_bindgen_test::wasm_bindgen_test(async)]
    async fn opfs_stream_budget_and_cancelled_read_seek() {
        let storage = OpfsStorage::new();
        let handle = StorageHandle::File("stream-tests/payload.bin".into());
        storage.write(&handle, b"abcdefgh").await.unwrap();
        let mut reader = storage.open_reader(&handle).await.unwrap();
        assert!(reader.buffered.is_empty());
        let mut byte = [0];
        // The first request is pending on the local browser task. Seeking
        // after cancelling it must discard bytes from its original offset.
        assert!(
            futures_lite::future::poll_once(reader.read(&mut byte))
                .await
                .is_none()
        );
        reader.seek(std::io::SeekFrom::End(-1)).await.unwrap();
        reader.read_exact(&mut byte).await.unwrap();
        assert_eq!(&byte, b"h");
        reader.seek(std::io::SeekFrom::Start(0)).await.unwrap();
        let mut bounded = (&mut reader).take(3);
        let mut bytes = Vec::new();
        bounded.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"abc");
        drop(bounded);
        assert_eq!(reader.buffered.len(), 3);
        assert_eq!(reader.position, 3);
    }
}

// NB: a `Storage` trait impl is intentionally absent — that trait's `Send`
// futures (`#[async_trait]` over `Send + Sync`) are incompatible with OPFS's
// non-`Send` JS futures. Callers use the inherent methods above under
// `spawn_local`. See the module docs.
