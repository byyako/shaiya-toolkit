# The SData wrapper

SData files (`*.SData`) hold the client's data tables (items, monsters, skills, quests ...). Some are stored **plain**, most are **wrapped**: SEED-encrypted with a fixed key behind a 64-byte header. The `shaiya-seed` crate reads and writes the wrapper.

## Layout of a wrapped file

```
signature  char[40]  "0001CBCEBC5B2784D3FC9A2A9DB84D1C3FEB6E99"
crc        u32       CRC-32 (the usual zlib CRC) of the plaintext
size       u32       plaintext length
padding    byte[16]  zero in the clients seen; kept as found
body       the plaintext, zero-padded to a multiple of 16 bytes, encrypted block by block
```

The cipher is KISA **SEED in ECB mode** with one fixed key; the lookup tables come from the public Parsec library (MIT licence, see `THIRD-PARTY-NOTICES.txt`). A file without the signature is plain.

## What the crate does

| Function | Behaviour |
|---|---|
| `is_encrypted(data)` | true when the data starts with the signature |
| `info(data, file)` | the header (`crc`, `size`, `padding`, body length), or `None` for plain data; errors on a short header, a body that is not a whole number of blocks, or a `size` that does not fit the body |
| `unwrap(data, file)` | the plaintext and a `Wrapper`; plain files pass through as `Wrapper::Plain`; a CRC mismatch is an error at byte 40 |
| `wrap(plain, &wrapper)` | the inverse: recomputes `crc` and `size`, reuses the header padding, reuses the **tail** when the length is unchanged |
| `decrypt` / `encrypt` | strict forms for the command line (`encrypt` writes zero padding and a zero tail) |
| `decrypt_unchecked` | the plaintext even when the checksum fails, and whether it matched (for inspection and repair) |

The *tail* is what the last block holds after `size` (the encoder's zero fill in most files). `Wrapper::Seed` keeps it so that `wrap(unwrap(x)) == x` holds **byte for byte** for every SData of every client tested; when the plaintext length changes, the tail is zero filled.

## Encrypted twice

A file is **encrypted twice** when the plaintext that `unwrap` gives is itself a wrapped file: it starts with the same 40-byte signature again. It happens when a wrapped file is run through an encryptor that does not look for the signature. A reader that removes one layer is left with encrypted bytes where it expects its table. The toolkit reports it as `sdata-double-encrypted` (see [Checks before files go in](../tools/shaiya.md#checks-before-files-go-in)); one `decrypt` gives the singly wrapped file back. A wrapper whose checksum fails, or whose header or block length is wrong, is `sdata-crc-bad`.

Verified on: synthetic files built by encrypting twice (the tests); the toolkit's opt-in test reports 0 originals with this finding on every client of the version matrix that has SData files.

## Notes

- Some loaders ignore a bad checksum, others reject it; the toolkit always checks it and offers `decrypt_unchecked` for damaged files.
- Some later episodes use a binary header for part of their data files; those files are handled by a later schema layer, not by the wrapper.
- Record layouts are not part of this page; they depend on the episode.
