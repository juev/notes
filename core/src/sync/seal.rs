//! S54, S55: a file storage encrypted with a passphrase.
//!
//! A random data key encrypts the files; the key itself lies in `vault.json`,
//! wrapped with a key derived from the passphrase. Each file is sealed with
//! XChaCha20-Poly1305 in chunks (the STREAM construction): the nonce of a chunk
//! is a random prefix of the file, the number of the chunk and a flag on the
//! last one, and its associated data is the path the file is read under.
//!
//! ```text
//! "LSE1" | prefix (19) | chunk 0 + tag | … | last chunk + tag
//! ```

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use chacha20poly1305::aead::{Aead, Generate, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::remote::Remote;
use crate::db::is_sha256;
use crate::error::{AppError, Result};
use crate::store::hex;

/// The plain bytes of one chunk; the last chunk of a file may be shorter, even empty.
pub(crate) const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
const MAGIC: &[u8; 4] = b"LSE1";
const PREFIX: usize = 19;
const HEADER: usize = MAGIC.len() + PREFIX;

/// S54: Argon2id with 64 MiB, three passes and one lane.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_PASSES: u32 = 3;
const SALT: usize = 16;

/// The bytes a file of `plain` bytes takes once sealed.
pub(crate) fn sealed_len(plain: u64) -> u64 {
    let chunks = plain.div_ceil(CHUNK as u64).max(1);
    HEADER as u64 + plain + chunks * TAG as u64
}

pub(crate) fn random<const N: usize>() -> [u8; N] {
    <[u8; N]>::generate()
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// The key derived from the passphrase: it opens only the wrapped data key.
fn derive(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let params = argon2::Params::new(KDF_MEMORY_KIB, KDF_PASSES, 1, Some(32))
        .map_err(|e| AppError::sync(format!("key derivation: {e}")))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| AppError::sync(format!("key derivation: {e}")))?;
    Ok(key)
}

/// The wrapped key is bound to its storage: a `vault.json` copied from
/// another storage does not open here.
fn wrap_aad(vault: &str) -> Vec<u8> {
    format!("lists vault {vault}").into_bytes()
}

/// What `vault.json` of format 4 holds besides the format and the vault id, in base64.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Wrapped {
    pub salt: String,
    pub nonce: String,
    pub key: String,
}

/// Wraps the data key under a key derived from `passphrase` with a new salt.
pub(crate) fn wrap(passphrase: &str, vault: &str, data_key: &[u8; 32]) -> Result<Wrapped> {
    let salt = random::<SALT>();
    let nonce = random::<24>();
    let kek = derive(passphrase, &salt)?;
    let sealed = XChaCha20Poly1305::new(&kek.into())
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: data_key,
                aad: &wrap_aad(vault),
            },
        )
        .map_err(|_| AppError::sync("the data key could not be wrapped"))?;
    Ok(Wrapped {
        salt: b64().encode(salt),
        nonce: b64().encode(nonce),
        key: b64().encode(sealed),
    })
}

/// The data key, or `None` when the passphrase does not open it.
pub(crate) fn unwrap(passphrase: &str, vault: &str, wrapped: &Wrapped) -> Result<Option<[u8; 32]>> {
    let unreadable = || AppError::sync("vault.json in the storage is not readable");
    let salt = b64().decode(&wrapped.salt).map_err(|_| unreadable())?;
    let nonce = b64().decode(&wrapped.nonce).map_err(|_| unreadable())?;
    let key = b64().decode(&wrapped.key).map_err(|_| unreadable())?;
    let nonce = XNonce::try_from(&nonce[..]).map_err(|_| unreadable())?;
    if salt.len() < 8 {
        return Err(unreadable());
    }
    let kek = derive(passphrase, &salt)?;
    let opened = XChaCha20Poly1305::new(&kek.into()).decrypt(
        &nonce,
        Payload {
            msg: &key,
            aad: &wrap_aad(vault),
        },
    );
    Ok(opened.ok().and_then(|k| <[u8; 32]>::try_from(k).ok()))
}

/// The keys taken from the data key: one seals the files, one names the attachment content.
pub(crate) struct Keys {
    file: XChaCha20Poly1305,
    name: [u8; 32],
}

fn subkey(data_key: &[u8; 32], label: &str) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(data_key).expect("any key length");
    mac.update(label.as_bytes());
    mac.finalize().into_bytes().into()
}

impl Keys {
    pub(crate) fn new(data_key: &[u8; 32]) -> Keys {
        Keys {
            file: XChaCha20Poly1305::new(&subkey(data_key, "lists file").into()),
            name: subkey(data_key, "lists blob name"),
        }
    }

    /// S55: attachment content lies under an HMAC of its hash, so the storage
    /// cannot tell whether it holds a file it knows.
    pub(crate) fn blob_name(&self, sha256: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.name).expect("any key length");
        mac.update(sha256.as_bytes());
        hex(&mac.finalize().into_bytes())
    }

    pub(crate) fn seal(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        let mut out = Sealer::new(self, path, Vec::with_capacity(sealed_len(data.len() as u64) as usize))?;
        out.write_all(data)?;
        out.finish()
    }

    /// `None` when the bytes were not sealed under this key and this path, or were changed.
    pub(crate) fn open(&self, path: &str, data: &[u8]) -> Option<Vec<u8>> {
        let mut out = Opener::new(self, path, Vec::with_capacity(data.len()), 0);
        out.write_all(data).ok()?;
        out.finish().ok()
    }
}

fn nonce(prefix: &[u8; PREFIX], counter: u32, last: bool) -> XNonce {
    let mut nonce = [0u8; 24];
    nonce[..PREFIX].copy_from_slice(prefix);
    nonce[PREFIX..PREFIX + 4].copy_from_slice(&counter.to_be_bytes());
    nonce[23] = u8::from(last);
    nonce.into()
}

fn overflow() -> std::io::Error {
    std::io::Error::other("the file has too many chunks")
}

/// Seals what is written to it into `out`, chunk by chunk.
struct Sealer<'a, W: Write> {
    keys: &'a Keys,
    aad: &'a [u8],
    prefix: [u8; PREFIX],
    counter: u32,
    buf: Vec<u8>,
    out: W,
}

impl<'a, W: Write> Sealer<'a, W> {
    fn new(keys: &'a Keys, path: &'a str, mut out: W) -> Result<Self> {
        let prefix = random::<PREFIX>();
        out.write_all(MAGIC)?;
        out.write_all(&prefix)?;
        Ok(Sealer {
            keys,
            aad: path.as_bytes(),
            prefix,
            counter: 0,
            buf: Vec::with_capacity(CHUNK + 1),
            out,
        })
    }

    fn chunk(&mut self, len: usize, last: bool) -> std::io::Result<()> {
        let sealed = self
            .keys
            .file
            .encrypt(
                &nonce(&self.prefix, self.counter, last),
                Payload {
                    msg: &self.buf[..len],
                    aad: self.aad,
                },
            )
            .map_err(|_| std::io::Error::other("a chunk could not be sealed"))?;
        self.out.write_all(&sealed)?;
        self.buf.drain(..len);
        self.counter = self.counter.checked_add(1).ok_or_else(overflow)?;
        Ok(())
    }

    fn finish(mut self) -> Result<W> {
        let len = self.buf.len();
        self.chunk(len, true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Sealer<'_, W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        // A full chunk is sealed as not the last only once a byte after it is known.
        while self.buf.len() > CHUNK {
            self.chunk(CHUNK, false)?;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Opens what is written to it into `out`. Only chunks that opened reach `out`;
/// a file cut short fails at `finish`, because its last chunk is not marked last.
pub(crate) struct Opener<'a, W: Write> {
    keys: &'a Keys,
    aad: &'a [u8],
    prefix: Option<[u8; PREFIX]>,
    counter: u32,
    buf: Vec<u8>,
    out: W,
    /// Plain bytes passed on, and how many may be; zero means no limit.
    written: u64,
    max: u64,
    pub(crate) too_large: bool,
}

fn refused() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "the file does not open under the key of the storage",
    )
}

impl<'a, W: Write> Opener<'a, W> {
    pub(crate) fn new(keys: &'a Keys, path: &'a str, out: W, max: u64) -> Self {
        Opener {
            keys,
            aad: path.as_bytes(),
            prefix: None,
            counter: 0,
            buf: Vec::new(),
            out,
            written: 0,
            max,
            too_large: false,
        }
    }

    fn chunk(&mut self, len: usize, last: bool) -> std::io::Result<()> {
        let prefix = self.prefix.as_ref().ok_or_else(refused)?;
        let plain = self
            .keys
            .file
            .decrypt(
                &nonce(prefix, self.counter, last),
                Payload {
                    msg: &self.buf[..len],
                    aad: self.aad,
                },
            )
            .map_err(|_| refused())?;
        self.buf.drain(..len);
        self.counter = self.counter.checked_add(1).ok_or_else(overflow)?;
        self.written += plain.len() as u64;
        if self.max != 0 && self.written > self.max {
            self.too_large = true;
            return Err(std::io::Error::other("the content is too large"));
        }
        self.out.write_all(&plain)
    }

    pub(crate) fn finish(mut self) -> std::io::Result<W> {
        if self.prefix.is_none() || self.buf.len() < TAG {
            return Err(refused());
        }
        let len = self.buf.len();
        self.chunk(len, true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Opener<'_, W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.prefix.is_none() {
            if self.buf.len() < HEADER {
                return Ok(data.len());
            }
            if &self.buf[..MAGIC.len()] != MAGIC {
                return Err(refused());
            }
            let mut prefix = [0u8; PREFIX];
            prefix.copy_from_slice(&self.buf[MAGIC.len()..HEADER]);
            self.prefix = Some(prefix);
            self.buf.drain(..HEADER);
        }
        while self.buf.len() > CHUNK + TAG {
            self.chunk(CHUNK + TAG, false)?;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// S55: the storage as sync sees it, with the files sealed on the way in and
/// opened on the way out. Sync reads and writes the same logical paths as
/// without encryption; only attachment content is stored under another name.
pub(crate) struct SealedRemote<'a> {
    inner: &'a dyn Remote,
    keys: Arc<Keys>,
}

impl<'a> SealedRemote<'a> {
    pub(crate) fn new(inner: &'a dyn Remote, keys: Arc<Keys>) -> Self {
        SealedRemote { inner, keys }
    }

    /// Where a logical path lies in the storage.
    pub(crate) fn physical(&self, path: &str) -> String {
        match path.rsplit_once('/') {
            Some((dir, name)) if dir.starts_with(&format!("{}/blobs/", super::ROOT)) && is_sha256(name) => {
                let hidden = self.keys.blob_name(name);
                format!("{}/blobs/{}/{hidden}", super::ROOT, &hidden[..2])
            }
            _ => path.to_string(),
        }
    }

    /// The marks in `log/` are empty files whose names say everything; they stay as they are.
    fn clear(path: &str) -> bool {
        path.ends_with(super::MARK) && path.starts_with(&format!("{}/log/", super::ROOT))
    }
}

impl Remote for SealedRemote<'_> {
    fn id(&self) -> String {
        self.inner.id()
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.list(dir)
    }

    /// A file that does not open reads as empty, which no reader takes: a log
    /// file stays unread (S8), an address is passed over.
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(data) = self.inner.get(&self.physical(path))? else {
            return Ok(None);
        };
        if Self::clear(path) {
            return Ok(Some(data));
        }
        Ok(Some(self.keys.open(path, &data).unwrap_or_default()))
    }

    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        if Self::clear(path) {
            return self.inner.put(path, data);
        }
        self.inner.put(&self.physical(path), &self.keys.seal(path, data)?)
    }

    /// R140: the file is sealed into a temporary file and sent from there, so it never sits in memory.
    fn put_file(&self, path: &str, local: &Path) -> Result<()> {
        let tmp = std::env::temp_dir().join(format!("lists-seal-{}.part", uuid::Uuid::now_v7().simple()));
        let sealed = (|| -> Result<()> {
            let out = std::io::BufWriter::new(super::private_create(&tmp)?);
            let mut sealer = Sealer::new(&self.keys, path, out)?;
            std::io::copy(&mut std::fs::File::open(local)?, &mut sealer)?;
            sealer.finish()?.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            self.inner.put_file(&self.physical(path), &tmp)
        })();
        let _ = std::fs::remove_file(&tmp);
        sealed
    }

    fn get_to(&self, path: &str, out: &mut dyn Write, max: u64) -> Result<bool> {
        let bound = if max == 0 { 0 } else { sealed_len(max) };
        let mut opener = Opener::new(&self.keys, path, out, max);
        let too_large = || AppError::too_large(format!("the content is more than {max} bytes"));
        match self.inner.get_to(&self.physical(path), &mut opener, bound) {
            Ok(false) => return Ok(false),
            Ok(true) => {}
            Err(AppError::TooLarge { .. }) => return Err(too_large()),
            Err(_) if opener.too_large => return Err(too_large()),
            Err(e) => return Err(e),
        }
        opener.finish().map(|_| true).map_err(|e| AppError::sync(e.to_string()))
    }

    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(&self.physical(path))
    }

    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(&self.physical(path))
    }

    fn folders(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.folders(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Keys {
        Keys::new(&[7u8; 32])
    }

    #[test]
    fn sealed_files_open_under_their_path_only() {
        let keys = keys();
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK, 3 * CHUNK + 5] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let sealed = keys.seal("lists/log/a", &data).unwrap();
            assert_eq!(sealed.len() as u64, sealed_len(len as u64), "{len}");
            assert_eq!(keys.open("lists/log/a", &sealed).as_deref(), Some(&data[..]), "{len}");
            assert!(keys.open("lists/log/b", &sealed).is_none(), "{len}");
            assert!(Keys::new(&[8u8; 32]).open("lists/log/a", &sealed).is_none(), "{len}");
        }
    }

    #[test]
    fn a_changed_or_cut_file_does_not_open() {
        let keys = keys();
        let data = vec![1u8; 2 * CHUNK + 10];
        let sealed = keys.seal("p", &data).unwrap();
        let mut changed = sealed.clone();
        changed[HEADER + 5] ^= 1;
        assert!(keys.open("p", &changed).is_none());
        // Cut at a chunk boundary: what is left is whole chunks, but none is marked last.
        assert!(keys.open("p", &sealed[..HEADER + CHUNK + TAG]).is_none());
        assert!(keys.open("p", &sealed[..sealed.len() - 1]).is_none());
        assert!(keys.open("p", b"").is_none());
        assert!(keys.open("p", &data).is_none());
    }

    #[test]
    fn opening_in_pieces_gives_the_same_bytes() {
        let keys = keys();
        let data: Vec<u8> = (0..(2 * CHUNK + 77)).map(|i| (i % 13) as u8).collect();
        let sealed = keys.seal("p", &data).unwrap();
        let mut opener = Opener::new(&keys, "p", Vec::new(), 0);
        for piece in sealed.chunks(1000) {
            opener.write_all(piece).unwrap();
        }
        assert_eq!(opener.finish().unwrap(), data);
        let mut limited = Opener::new(&keys, "p", Vec::new(), CHUNK as u64);
        assert!(limited.write_all(&sealed).is_err() || limited.finish().is_err());
    }

    #[test]
    fn the_data_key_opens_with_its_passphrase_and_vault_only() {
        let key = random::<32>();
        let wrapped = wrap("correct horse", "vault-1", &key).unwrap();
        assert_eq!(unwrap("correct horse", "vault-1", &wrapped).unwrap(), Some(key));
        assert_eq!(unwrap("wrong", "vault-1", &wrapped).unwrap(), None);
        assert_eq!(unwrap("correct horse", "vault-2", &wrapped).unwrap(), None);
    }

    /// S54, S55: bytes sealed by argon2 0.5 and chacha20poly1305 0.10 still open,
    /// so a storage written before an update of these crates stays readable.
    #[test]
    fn a_storage_sealed_by_an_earlier_release_opens() {
        let wrapped = Wrapped {
            salt: "twtGQyiD105Yaq3g3B5Phw==".into(),
            nonce: "obn7Cn5iigKS1jiv47F+uwOETC7W1AK7".into(),
            key: "rzWYUxzZPr/g4OUd4IMoPZdaFIjrORQ3R763niMM4D21+rXO+htW02f9e0i0IQeE".into(),
        };
        assert_eq!(unwrap("correct horse", "vault-1", &wrapped).unwrap(), Some([5u8; 32]));
        let sealed = b64()
            .decode("TFNFMRNpKks1CrpKdMwoBoeg0ytb63pE6ZgEn/wFQw11N15Kb6gG1Za+LbAapcJRYHsuBYiPSUUGSpkl")
            .unwrap();
        let opened = Keys::new(&[7u8; 32]).open("lists/log/a", &sealed);
        assert_eq!(opened.as_deref(), Some(&b"the first sealed file"[..]));
    }

    #[test]
    fn blob_names_hide_the_hash() {
        let sha = "ab".repeat(32);
        let name = keys().blob_name(&sha);
        assert!(is_sha256(&name));
        assert_ne!(name, sha);
        assert_ne!(name, Keys::new(&[8u8; 32]).blob_name(&sha));
    }
}
