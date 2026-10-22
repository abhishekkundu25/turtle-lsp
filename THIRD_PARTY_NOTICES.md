# Third-party notices

turtle-lsp is under the MIT licence (`LICENSE`). The files it ships include
work of others, listed here with their licences:

- the `turtle-lsp` binary (release archives, the `.vsix`, `build.lua`'s
  download): the Rust crates below, the bundled JSON-LD contexts and the
  vocabulary data;
- the VS Code extension (`.vsix`): the binary, and the npm packages bundled
  into `out/extension.js`;
- the browser site (`web/app`): the engine (`engine.wasm`, built from the
  crates marked "browser", with the same contexts and vocabulary data),
  Monaco and monaco-vim.

Where a crate offers a choice of licences, turtle-lsp uses it under the MIT
licence, or under Apache-2.0 where MIT is not offered (`ring`: Apache-2.0
and ISC).

To check this list against the code, compare it with the crates that
`cargo tree --locked -e normal,no-proc-macro --prefix none --target all -f '{p} {l}'`
prints at the repository root (and in `web/engine` with
`--target wasm32-unknown-unknown`). Crates that only run at build time
(procedural macros and their dependencies) are not part of what ships and
are not listed.

## Rust crates

"binary" is the `turtle-lsp` binary. Some crates are in the build for one
platform only: `windows-*` on Windows, and `r-efi`, `wasi`, `wasip2` and
`wit-bindgen` on none of the release platforms. "browser" is the engine of
the browser site. `spareval`
is vendored with a patch (`vendor/spareval/PATCH.md`).

| Crate | Version | Licence | Copyright | In |
|---|---|---|---|---|
| [aho-corasick](https://github.com/BurntSushi/aho-corasick) | 1.1.5 | Unlicense OR MIT | Copyright (c) 2015 Andrew Gallant | binary, browser |
| [base64](https://github.com/marshallpierce/rust-base64) | 0.23.1 | MIT OR Apache-2.0 | Copyright (c) 2025 Alice Maz, Marshall Pierce | binary |
| [bitflags](https://github.com/bitflags/bitflags) | 1.3.2 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers | binary, browser |
| [block-buffer](https://github.com/RustCrypto/utils) | 0.12.1 | MIT OR Apache-2.0 | Copyright (c) 2018-2025 The RustCrypto Project Developers | binary, browser |
| [bytes](https://github.com/tokio-rs/bytes) | 1.12.1 | MIT | Copyright (c) 2018 Carl Lerche | binary |
| [cfg-if](https://github.com/rust-lang/cfg-if) | 1.0.5 | MIT OR Apache-2.0 | Copyright (c) 2014 Alex Crichton | binary, browser |
| [const-oid](https://github.com/RustCrypto/formats) | 0.10.2 | Apache-2.0 OR MIT | Copyright (c) 2020-2026 The RustCrypto Project Developers | binary, browser |
| [cpufeatures](https://github.com/RustCrypto/utils) | 0.3.1 | MIT OR Apache-2.0 | Copyright (c) 2020-2026 The RustCrypto Project Developers | binary |
| [crossbeam-channel](https://github.com/crossbeam-rs/crossbeam) | 0.5.17 | MIT OR Apache-2.0 | Copyright (c) 2019 The Crossbeam Project Developers | binary, browser |
| [crossbeam-deque](https://github.com/crossbeam-rs/crossbeam) | 0.8.8 | MIT OR Apache-2.0 | Copyright (c) 2019 The Crossbeam Project Developers | binary, browser |
| [crossbeam-epoch](https://github.com/crossbeam-rs/crossbeam) | 0.9.21 | MIT OR Apache-2.0 | Copyright (c) 2019 The Crossbeam Project Developers | binary, browser |
| [crossbeam-utils](https://github.com/crossbeam-rs/crossbeam) | 0.8.23 | MIT OR Apache-2.0 | Copyright (c) 2019 The Crossbeam Project Developers | binary, browser |
| [crypto-common](https://github.com/RustCrypto/traits) | 0.2.2 | MIT OR Apache-2.0 | Copyright (c) 2021-2026 RustCrypto Developers | binary, browser |
| [digest](https://github.com/RustCrypto/traits) | 0.11.3 | MIT OR Apache-2.0 | Copyright (c) 2017 Artyom Pavlov; Copyright (c) 2017-2025 RustCrypto Developers | binary, browser |
| [either](https://github.com/rayon-rs/either) | 1.19.0 | MIT OR Apache-2.0 | the either authors | binary, browser |
| [fluent-uri](https://github.com/yescallop/fluent-uri-rs) | 0.1.4 | MIT | Copyright (c) 2021 Scallop Ye | binary, browser |
| [form_urlencoded](https://github.com/servo/rust-url) | 1.2.2 | MIT OR Apache-2.0 | Copyright (c) 2013-2016 The rust-url developers | binary, browser |
| [getrandom](https://github.com/rust-random/getrandom) | 0.2.17 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers; Copyright (c) 2018-2024 The rust-random Project Developers | binary |
| [getrandom](https://github.com/rust-random/getrandom) | 0.3.4 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers; Copyright (c) 2018-2025 The rust-random Project Developers | binary, browser |
| [hex](https://github.com/KokaKiwi/rust-hex) | 0.4.3 | MIT OR Apache-2.0 | Copyright (c) 2013-2014 The Rust Project Developers.; Copyright (c) 2015-2020 The rust-hex Developers | binary, browser |
| [http](https://github.com/hyperium/http) | 1.5.0 | MIT OR Apache-2.0 | Copyright (c) 2017 http-rs authors | binary |
| [httparse](https://github.com/seanmonstar/httparse) | 1.10.1 | MIT OR Apache-2.0 | Copyright (c) 2015-2025 Sean McArthur | binary |
| [hybrid-array](https://github.com/RustCrypto/hybrid-array) | 0.4.15 | MIT OR Apache-2.0 | Copyright (c) 2022-2026 The RustCrypto Project Developers | binary, browser |
| [icu_collections](https://github.com/unicode-org/icu4x) | 2.3.0 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [icu_locale_core](https://github.com/unicode-org/icu4x) | 2.3.0 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [icu_normalizer](https://github.com/unicode-org/icu4x) | 2.3.0 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [icu_normalizer_data](https://github.com/unicode-org/icu4x) | 2.3.0 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [icu_properties](https://github.com/unicode-org/icu4x) | 2.3.0 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [icu_properties_data](https://github.com/unicode-org/icu4x) | 2.3.0 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [icu_provider](https://github.com/unicode-org/icu4x) | 2.3.1 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [idna](https://github.com/servo/rust-url/) | 1.1.0 | MIT OR Apache-2.0 | Copyright (c) 2013-2025 The rust-url developers | binary, browser |
| [idna_adapter](https://github.com/hsivonen/idna_adapter) | 1.2.2 | Apache-2.0 OR MIT | Copyright (c) The rust-url developers | binary, browser |
| [itoa](https://github.com/dtolnay/itoa) | 1.0.18 | MIT OR Apache-2.0 | David Tolnay | binary, browser |
| [json-event-parser](https://github.com/oxigraph/json-event-parser) | 0.2.3 | MIT OR Apache-2.0 | Copyright (c) 2018 Oxigraph developers | binary, browser |
| [libc](https://github.com/rust-lang/libc) | 0.2.190 | MIT OR Apache-2.0 | Copyright (c) The Rust Project Developers | binary |
| [libmimalloc-sys](https://github.com/purpleprotocol/mimalloc_rust/tree/master/libmimalloc-sys) | 0.1.49 | MIT | Copyright 2019 Octavian Oncescu | binary |
| [line-index](https://github.com/rust-lang/rust-analyzer/tree/master/lib/line-index) | 0.1.2 | MIT OR Apache-2.0 | the line-index authors | binary, browser |
| [litemap](https://github.com/unicode-org/icu4x) | 0.8.3 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [log](https://github.com/rust-lang/log) | 0.4.34 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers | binary, browser |
| [lsp-server](https://github.com/rust-lang/rust-analyzer/tree/master/lib/lsp-server) | 0.10.0 | MIT OR Apache-2.0 | the lsp-server authors | binary, browser |
| [lsp-types](https://github.com/gluon-lang/lsp-types) | 0.97.0 | MIT | Copyright (c) 2016 Markus Westerlind | binary, browser |
| [md-5](https://github.com/RustCrypto/hashes) | 0.11.0 | MIT OR Apache-2.0 | Copyright (c) 2006-2009 Graydon Hoare; Copyright (c) 2009-2013 Mozilla Foundation; Copyright (c) 2016 Artyom Pavlov | binary, browser |
| [memchr](https://github.com/BurntSushi/memchr) | 2.8.3 | Unlicense OR MIT | Copyright (c) 2015 Andrew Gallant | binary, browser |
| [mimalloc](https://github.com/purpleprotocol/mimalloc_rust) | 0.1.52 | MIT | Copyright 2019 Octavian Oncescu | binary |
| [nohash-hasher](https://github.com/paritytech/nohash-hasher) | 0.2.0 | Apache-2.0 OR MIT | Copyright 2018 Parity Technologies (UK) Ltd. | binary, browser |
| [once_cell](https://github.com/matklad/once_cell) | 1.21.4 | MIT OR Apache-2.0 | Aleksey Kladov | binary |
| [oxilangtag](https://github.com/oxigraph/oxilangtag) | 0.1.6 | MIT | Copyright (c) 2015-2021 Pyfisch Tpt | binary, browser |
| [oxiri](https://github.com/oxigraph/oxiri) | 0.2.11 | MIT OR Apache-2.0 | Copyright (c) 2018 Oxigraph developers | binary, browser |
| [oxiri](https://github.com/oxigraph/oxiri) | 0.3.1 | MIT OR Apache-2.0 | Copyright (c) 2018 Oxigraph developers | binary, browser |
| [oxrdf](https://github.com/oxigraph/oxigraph/tree/main/lib/oxrdf) | 0.3.4 | MIT OR Apache-2.0 | Tpt | binary, browser |
| [oxrdfxml](https://github.com/oxigraph/oxigraph/tree/master/lib/oxrdfxml) | 0.2.4 | MIT OR Apache-2.0 | Tpt | binary, browser |
| [oxsdatatypes](https://github.com/oxigraph/oxigraph/tree/main/lib/oxsdatatypes) | 0.2.3 | MIT OR Apache-2.0 | Tpt | binary, browser |
| [peg](https://github.com/kevinmehall/rust-peg) | 0.8.6 | MIT | Copyright (C) 2013 Kevin Mehall | binary, browser |
| [peg-runtime](https://github.com/kevinmehall/rust-peg) | 0.8.6 | MIT | Copyright (C) 2013 Kevin Mehall | binary, browser |
| [percent-encoding](https://github.com/servo/rust-url/) | 2.3.2 | MIT OR Apache-2.0 | Copyright (c) 2013-2025 The rust-url developers | binary, browser |
| [potential_utf](https://github.com/unicode-org/icu4x) | 0.1.6 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [ppv-lite86](https://github.com/cryptocorrosion/cryptocorrosion) | 0.2.21 | MIT OR Apache-2.0 | Copyright (c) 2019 The CryptoCorrosion Contributors | binary, browser |
| [quick-xml](https://github.com/tafia/quick-xml) | 0.37.5 | MIT | Copyright (c) 2016 Johann Tuffe | binary, browser |
| [r-efi](https://github.com/r-efi/r-efi) | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | the r-efi authors | binary |
| [rand](https://github.com/rust-random/rand) | 0.9.5 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers; Copyright 2018 Developers of the Rand project | binary, browser |
| [rand_chacha](https://github.com/rust-random/rand) | 0.9.0 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers; Copyright 2018 Developers of the Rand project | binary, browser |
| [rand_core](https://github.com/rust-random/rand) | 0.9.5 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers; Copyright 2018 Developers of the Rand project | binary, browser |
| [rayon](https://github.com/rayon-rs/rayon) | 1.12.0 | MIT OR Apache-2.0 | Copyright (c) 2010 The Rust Project Developers | binary, browser |
| [rayon-core](https://github.com/rayon-rs/rayon) | 1.13.0 | MIT OR Apache-2.0 | Copyright (c) 2010 The Rust Project Developers | binary, browser |
| [regex](https://github.com/rust-lang/regex) | 1.13.1 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers | binary, browser |
| [regex-automata](https://github.com/rust-lang/regex) | 0.4.18 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers | binary, browser |
| [regex-syntax](https://github.com/rust-lang/regex) | 0.8.11 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers | binary, browser |
| [ring](https://github.com/briansmith/ring) | 0.17.14 | Apache-2.0 AND ISC | Copyright 2015-2025 Brian Smith. | binary |
| [rustc-hash](https://github.com/rust-lang/rustc-hash) | 2.1.3 | Apache-2.0 OR MIT | The Rust Project Developers | binary, browser |
| [rustls](https://github.com/rustls/rustls) | 0.23.46 | Apache-2.0 OR ISC OR MIT | Copyright (c) 2016 Joseph Birr-Pixton <jpixton@gmail.com> | binary |
| [rustls-pki-types](https://github.com/rustls/pki-types) | 1.15.1 | MIT OR Apache-2.0 | Copyright (c) 2023 Dirkjan Ochtman <dirkjan@ochtman.nl>; Copyright 2023 Dirkjan Ochtman | binary |
| [rustls-webpki](https://github.com/rustls/webpki) | 0.103.16 | ISC | Copyright 2015 Brian Smith. | binary |
| [serde](https://github.com/serde-rs/serde) | 1.0.229 | MIT OR Apache-2.0 | Erick Tryzelaar, David Tolnay | binary, browser |
| [serde_core](https://github.com/serde-rs/serde) | 1.0.229 | MIT OR Apache-2.0 | Erick Tryzelaar, David Tolnay | binary, browser |
| [serde_json](https://github.com/serde-rs/json) | 1.0.154 | MIT OR Apache-2.0 | the serde_json authors | binary, browser |
| [sha1](https://github.com/RustCrypto/hashes) | 0.11.0 | MIT OR Apache-2.0 | Copyright (c) 2006-2009 Graydon Hoare; Copyright (c) 2009-2013 Mozilla Foundation; Copyright (c) 2016 Artyom Pavlov | binary, browser |
| [sha2](https://github.com/RustCrypto/hashes) | 0.11.1 | MIT OR Apache-2.0 | Copyright (c) 2006-2009 Graydon Hoare; Copyright (c) 2009-2013 Mozilla Foundation; Copyright (c) 2016 Artyom Pavlov | binary, browser |
| [smallvec](https://github.com/servo/rust-smallvec) | 1.16.3 | MIT OR Apache-2.0 | Copyright (c) 2018 The Servo Project Developers | binary, browser |
| [sparesults](https://github.com/oxigraph/oxigraph/tree/main/lib/sparesults) | 0.3.4 | MIT OR Apache-2.0 | Tpt | binary, browser |
| [spareval](https://github.com/oxigraph/oxigraph/tree/main/lib/spareval) | 0.2.7 | MIT OR Apache-2.0 | Tpt | binary, browser |
| [spargebra](https://github.com/oxigraph/oxigraph/tree/main/lib/spargebra) | 0.4.7 | MIT OR Apache-2.0 | Tpt | binary, browser |
| [sparopt](https://github.com/oxigraph/oxigraph/tree/main/lib/sparopt) | 0.3.7 | MIT OR Apache-2.0 | Tpt | binary, browser |
| [stable_deref_trait](https://github.com/storyyeller/stable_deref_trait) | 1.2.1 | MIT OR Apache-2.0 | Copyright (c) 2017 Robert Grosse | binary, browser |
| [subtle](https://github.com/dalek-cryptography/subtle) | 2.6.1 | BSD-3-Clause | Copyright (c) 2016-2017 Isis Agora Lovecruft, Henry de Valence. All rights reserved.; Copyright (c) 2016-2024 Isis Agora Lovecruft. All rights reserved. | binary |
| [text-size](https://github.com/rust-analyzer/text-size) | 1.1.1 | MIT OR Apache-2.0 | Aleksey Kladov, Christopher Durham (CAD97) | binary, browser |
| [thiserror](https://github.com/dtolnay/thiserror) | 2.0.21 | MIT OR Apache-2.0 | David Tolnay | binary, browser |
| [tinystr](https://github.com/unicode-org/icu4x) | 0.8.4 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [typenum](https://github.com/paholg/typenum) | 1.20.1 | MIT OR Apache-2.0 | Copyright (c) 2014 Paho Lurie-Gregg | binary, browser |
| [untrusted](https://github.com/briansmith/untrusted) | 0.9.0 | ISC | Brian Smith | binary |
| [ureq](https://github.com/algesten/ureq) | 3.4.2 | MIT OR Apache-2.0 | Copyright (c) 2019 Martin Algesten | binary |
| [ureq-proto](https://github.com/algesten/ureq-proto) | 0.6.4 | MIT OR Apache-2.0 | Copyright 2022 Martin Algesten | binary |
| [url](https://github.com/servo/rust-url) | 2.5.8 | MIT OR Apache-2.0 | Copyright (c) 2013-2025 The rust-url developers | binary, browser |
| [utf8_iter](https://github.com/hsivonen/utf8_iter) | 1.0.4 | Apache-2.0 OR MIT | Copyright Mozilla Foundation | binary, browser |
| [utf8-zero](https://github.com/algesten/utf8-zero) | 0.8.1 | MIT OR Apache-2.0 | Simon Sapin, Martin Algesten | binary |
| [wasi](https://github.com/bytecodealliance/wasi) | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | The Cranelift Project Developers | binary |
| [wasip2](https://github.com/bytecodealliance/wasi-rs) | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | the wasip2 authors | binary |
| [webpki-roots](https://github.com/rustls/webpki-roots) | 1.0.10 | CDLA-Permissive-2.0 | the webpki-roots authors | binary |
| [windows_aarch64_gnullvm](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows_aarch64_msvc](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows_i686_gnu](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows_i686_gnullvm](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows_i686_msvc](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows_x86_64_gnu](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows_x86_64_gnullvm](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows_x86_64_msvc](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows-sys](https://github.com/microsoft/windows-rs) | 0.52.0 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [windows-targets](https://github.com/microsoft/windows-rs) | 0.52.6 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation. | binary |
| [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen) | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Alex Crichton | binary |
| [writeable](https://github.com/unicode-org/icu4x) | 0.6.4 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [yoke](https://github.com/unicode-org/icu4x) | 0.8.3 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [zerocopy](https://github.com/google/zerocopy) | 0.8.62 | BSD-2-Clause OR Apache-2.0 OR MIT | Copyright 2019 The Fuchsia Authors.; Copyright 2023 The Fuchsia Authors | binary, browser |
| [zerofrom](https://github.com/unicode-org/icu4x) | 0.1.8 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [zeroize](https://github.com/RustCrypto/utils) | 1.9.1 | Apache-2.0 OR MIT | Copyright (c) 2018-2026 The RustCrypto Project Developers | binary |
| [zerotrie](https://github.com/unicode-org/icu4x) | 0.2.5 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [zerovec](https://github.com/unicode-org/icu4x) | 0.11.8 | Unicode-3.0 | Copyright © 2020-2024 Unicode, Inc. | binary, browser |
| [zmij](https://github.com/dtolnay/zmij) | 1.0.23 | MIT | David Tolnay | binary, browser |

## Data

### JSON-LD contexts (`data/jsonld-contexts`)

Unmodified copies, compiled into the binary and the browser engine. Sources
and dates are in `data/jsonld-contexts/README.md`.

| File | Source | Licence |
|---|---|---|
| `schema-org.jsonld` | https://schema.org/docs/jsonldcontext.jsonld, by Schema.org | [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/) |
| `activitystreams.jsonld` | https://www.w3.org/ns/activitystreams, by W3C | [W3C Software and Document License](https://www.w3.org/copyright/software-license-2023/) |
| `credentials-v1.jsonld` | https://www.w3.org/2018/credentials/v1, by W3C | [W3C Software and Document License](https://www.w3.org/copyright/software-license-2023/) |
| `credentials-v2.jsonld` | https://www.w3.org/ns/credentials/v2, by W3C | [W3C Software and Document License](https://www.w3.org/copyright/software-license-2023/) |
| `did-v1.jsonld` | https://www.w3.org/ns/did/v1, by W3C | [W3C Software and Document License](https://www.w3.org/copyright/software-license-2023/) |

The W3C files are Copyright © World Wide Web Consortium (MIT, ERCIM, Keio,
Beihang).

### Vocabularies (`data/vocab.tsv`)

The names, labels, comments and class and property relations of
83 vocabularies, extracted by `tools/build-vocab.mjs` from
[@zazuko/rdf-vocabularies](https://github.com/zazuko/rdf-vocabularies)
2023.1.19 (MIT, Copyright (c) 2019 Zazuko GmbH). Each vocabulary keeps the
licence its publisher gives it; the namespace of each is the `@ns` line in
`data/vocab.tsv`. The prefixes: `acl`, `as`, `bibo`, `cc`, `cnt`, `constant`, `crm`, `csvw`, `dash`, `dbo`, `dc11`, `dcam`, `dcat`, `dcmitype`, `dcterms`, `discipline`, `doap`, `dpv`, `dqv`, `dtype`, `duv`, `earl`, `ebucore`, `exif`, `foaf`, `frbr`, `geo`, `geof`, `geor`, `gn`, `gr`, `grddl`, `gs1`, `gtfs`, `http`, `hydra`, `ical`, `ldp`, `locn`, `lvont`, `ma`, `mads`, `oa`, `og`, `org`, `owl`, `prefix`, `prov`, `qb`, `qkdv`, `quantitykind`, `qudt`, `rdau`, `rdf`, `rdfa`, `rdfs`, `rico`, `rr`, `rss`, `schema`, `sd`, `sdmx`, `sem`, `sh`, `shex`, `sioc`, `skos`, `skosxl`, `sosa`, `sou`, `ssn`, `test`, `time`, `unit`, `vaem`, `vann`, `vcard`, `void`, `vs`, `wgs`, `xhv`, `xkos`, `xsd`.

## npm packages in the VS Code extension

Bundled into `out/extension.js` by esbuild (`npm ls --omit=dev --all` in
`editors/vscode`).

| Package | Version | Licence | Copyright |
|---|---|---|---|
| vscode-languageclient | 10.1.2 | MIT | Copyright (c) Microsoft Corporation |
| vscode-languageserver-protocol | 3.18.4 | MIT | Copyright (c) Microsoft Corporation |
| vscode-languageserver-types | 3.18.4 | MIT | Copyright (c) Microsoft Corporation |
| vscode-languageserver-textdocument | 1.0.15 | MIT | Copyright (c) Microsoft Corporation |
| vscode-jsonrpc | 9.0.3 | MIT | Copyright (c) Microsoft Corporation |
| minimatch | 10.2.6 | BlueOak-1.0.0 | Isaac Z. Schlueter and contributors |
| brace-expansion | 5.0.12 | MIT | Copyright Julian Gruber |
| balanced-match | 4.0.4 | MIT | Original code Copyright Julian Gruber; port to TypeScript Copyright Isaac Z. Schlueter |
| semver | 7.8.5 | ISC | Copyright (c) Isaac Z. Schlueter and Contributors |

## The browser site

- [Monaco Editor](https://github.com/microsoft/monaco-editor) 0.52.2, MIT,
  Copyright (c) 2016 - present Microsoft Corporation. Its `LICENSE` and
  `ThirdPartyNotices.txt` (for the code Monaco bundles) are published beside
  it, in `vendor/monaco/`.
- [monaco-vim](https://github.com/brijeshb42/monaco-vim) 0.4.4, MIT,
  Copyright 2018 Brijesh Bittu, with code from CodeMirror, MIT, Copyright (C)
  2017 by Marijn Haverbeke and others. Both licences are published beside
  it, in `vendor/monaco-vim/`.

## Licence texts

### MIT

Each MIT-licensed item above is under this text, with its own copyright
line from the tables above.

````text
Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
````

### Apache-2.0

````text
                              Apache License
                        Version 2.0, January 2004
                     http://www.apache.org/licenses/

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1. Definitions.

   "License" shall mean the terms and conditions for use, reproduction,
   and distribution as defined by Sections 1 through 9 of this document.

   "Licensor" shall mean the copyright owner or entity authorized by
   the copyright owner that is granting the License.

   "Legal Entity" shall mean the union of the acting entity and all
   other entities that control, are controlled by, or are under common
   control with that entity. For the purposes of this definition,
   "control" means (i) the power, direct or indirect, to cause the
   direction or management of such entity, whether by contract or
   otherwise, or (ii) ownership of fifty percent (50%) or more of the
   outstanding shares, or (iii) beneficial ownership of such entity.

   "You" (or "Your") shall mean an individual or Legal Entity
   exercising permissions granted by this License.

   "Source" form shall mean the preferred form for making modifications,
   including but not limited to software source code, documentation
   source, and configuration files.

   "Object" form shall mean any form resulting from mechanical
   transformation or translation of a Source form, including but
   not limited to compiled object code, generated documentation,
   and conversions to other media types.

   "Work" shall mean the work of authorship, whether in Source or
   Object form, made available under the License, as indicated by a
   copyright notice that is included in or attached to the work
   (an example is provided in the Appendix below).

   "Derivative Works" shall mean any work, whether in Source or Object
   form, that is based on (or derived from) the Work and for which the
   editorial revisions, annotations, elaborations, or other modifications
   represent, as a whole, an original work of authorship. For the purposes
   of this License, Derivative Works shall not include works that remain
   separable from, or merely link (or bind by name) to the interfaces of,
   the Work and Derivative Works thereof.

   "Contribution" shall mean any work of authorship, including
   the original version of the Work and any modifications or additions
   to that Work or Derivative Works thereof, that is intentionally
   submitted to Licensor for inclusion in the Work by the copyright owner
   or by an individual or Legal Entity authorized to submit on behalf of
   the copyright owner. For the purposes of this definition, "submitted"
   means any form of electronic, verbal, or written communication sent
   to the Licensor or its representatives, including but not limited to
   communication on electronic mailing lists, source code control systems,
   and issue tracking systems that are managed by, or on behalf of, the
   Licensor for the purpose of discussing and improving the Work, but
   excluding communication that is conspicuously marked or otherwise
   designated in writing by the copyright owner as "Not a Contribution."

   "Contributor" shall mean Licensor and any individual or Legal Entity
   on behalf of whom a Contribution has been received by Licensor and
   subsequently incorporated within the Work.

2. Grant of Copyright License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   copyright license to reproduce, prepare Derivative Works of,
   publicly display, publicly perform, sublicense, and distribute the
   Work and such Derivative Works in Source or Object form.

3. Grant of Patent License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   (except as stated in this section) patent license to make, have made,
   use, offer to sell, sell, import, and otherwise transfer the Work,
   where such license applies only to those patent claims licensable
   by such Contributor that are necessarily infringed by their
   Contribution(s) alone or by combination of their Contribution(s)
   with the Work to which such Contribution(s) was submitted. If You
   institute patent litigation against any entity (including a
   cross-claim or counterclaim in a lawsuit) alleging that the Work
   or a Contribution incorporated within the Work constitutes direct
   or contributory patent infringement, then any patent licenses
   granted to You under this License for that Work shall terminate
   as of the date such litigation is filed.

4. Redistribution. You may reproduce and distribute copies of the
   Work or Derivative Works thereof in any medium, with or without
   modifications, and in Source or Object form, provided that You
   meet the following conditions:

   (a) You must give any other recipients of the Work or
       Derivative Works a copy of this License; and

   (b) You must cause any modified files to carry prominent notices
       stating that You changed the files; and

   (c) You must retain, in the Source form of any Derivative Works
       that You distribute, all copyright, patent, trademark, and
       attribution notices from the Source form of the Work,
       excluding those notices that do not pertain to any part of
       the Derivative Works; and

   (d) If the Work includes a "NOTICE" text file as part of its
       distribution, then any Derivative Works that You distribute must
       include a readable copy of the attribution notices contained
       within such NOTICE file, excluding those notices that do not
       pertain to any part of the Derivative Works, in at least one
       of the following places: within a NOTICE text file distributed
       as part of the Derivative Works; within the Source form or
       documentation, if provided along with the Derivative Works; or,
       within a display generated by the Derivative Works, if and
       wherever such third-party notices normally appear. The contents
       of the NOTICE file are for informational purposes only and
       do not modify the License. You may add Your own attribution
       notices within Derivative Works that You distribute, alongside
       or as an addendum to the NOTICE text from the Work, provided
       that such additional attribution notices cannot be construed
       as modifying the License.

   You may add Your own copyright statement to Your modifications and
   may provide additional or different license terms and conditions
   for use, reproduction, or distribution of Your modifications, or
   for any such Derivative Works as a whole, provided Your use,
   reproduction, and distribution of the Work otherwise complies with
   the conditions stated in this License.

5. Submission of Contributions. Unless You explicitly state otherwise,
   any Contribution intentionally submitted for inclusion in the Work
   by You to the Licensor shall be under the terms and conditions of
   this License, without any additional terms or conditions.
   Notwithstanding the above, nothing herein shall supersede or modify
   the terms of any separate license agreement you may have executed
   with Licensor regarding such Contributions.

6. Trademarks. This License does not grant permission to use the trade
   names, trademarks, service marks, or product names of the Licensor,
   except as required for reasonable and customary use in describing the
   origin of the Work and reproducing the content of the NOTICE file.

7. Disclaimer of Warranty. Unless required by applicable law or
   agreed to in writing, Licensor provides the Work (and each
   Contributor provides its Contributions) on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
   implied, including, without limitation, any warranties or conditions
   of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
   PARTICULAR PURPOSE. You are solely responsible for determining the
   appropriateness of using or redistributing the Work and assume any
   risks associated with Your exercise of permissions under this License.

8. Limitation of Liability. In no event and under no legal theory,
   whether in tort (including negligence), contract, or otherwise,
   unless required by applicable law (such as deliberate and grossly
   negligent acts) or agreed to in writing, shall any Contributor be
   liable to You for damages, including any direct, indirect, special,
   incidental, or consequential damages of any character arising as a
   result of this License or out of the use or inability to use the
   Work (including but not limited to damages for loss of goodwill,
   work stoppage, computer failure or malfunction, or any and all
   other commercial damages or losses), even if such Contributor
   has been advised of the possibility of such damages.

9. Accepting Warranty or Additional Liability. While redistributing
   the Work or Derivative Works thereof, You may choose to offer,
   and charge a fee for, acceptance of support, warranty, indemnity,
   or other liability obligations and/or rights consistent with this
   License. However, in accepting such obligations, You may act only
   on Your own behalf and on Your sole responsibility, not on behalf
   of any other Contributor, and only if You agree to indemnify,
   defend, and hold each Contributor harmless for any liability
   incurred by, or claims asserted against, such Contributor by reason
   of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS

APPENDIX: How to apply the Apache License to your work.

   To apply the Apache License to your work, attach the following
   boilerplate notice, with the fields enclosed by brackets "[]"
   replaced with your own identifying information. (Don't include
   the brackets!)  The text should be enclosed in the appropriate
   comment syntax for the file format. We also recommend that a
   file or class name and description of purpose be included on the
   same "printed page" as the copyright notice for easier
   identification within third-party archives.

Copyright [yyyy] [name of copyright owner]

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

	http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
````

### ISC

`ring` (Copyright 2015-2025 Brian Smith), `rustls-webpki` (Copyright 2015
Brian Smith), `untrusted` (Brian Smith) and `semver` are under this text,
with their own copyright lines.

````text
Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
````

### BSD-3-Clause

`subtle`:

````text
Copyright (c) 2016-2017 Isis Agora Lovecruft, Henry de Valence. All rights reserved.
Copyright (c) 2016-2024 Isis Agora Lovecruft. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE. 
````

### Unicode-3.0

The ICU4X crates (`icu_*`, `litemap`, `potential_utf`, `tinystr`,
`writeable`, `yoke`, `zerofrom`, `zerotrie`, `zerovec`):

````text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 2020-2024 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.

SPDX-License-Identifier: Unicode-3.0

—

Portions of ICU4X may have been adapted from ICU4C and/or ICU4J.
ICU 1.8.1 to ICU 57.1 © 1995-2016 International Business Machines Corporation and others.
````

### CDLA-Permissive-2.0

`webpki-roots`, whose data is the list of root certificates that the HTTPS
client trusts:

````text
# Community Data License Agreement - Permissive - Version 2.0

This is the Community Data License Agreement - Permissive, Version
2.0 (the "agreement"). Data Provider(s) and Data Recipient(s) agree
as follows:

## 1. Provision of the Data

1.1. A Data Recipient may use, modify, and share the Data made
available by Data Provider(s) under this agreement if that Data
Recipient follows the terms of this agreement.

1.2. This agreement does not impose any restriction on a Data
Recipient's use, modification, or sharing of any portions of the
Data that are in the public domain or that may be used, modified,
or shared under any other legal exception or limitation.

## 2. Conditions for Sharing Data

2.1. A Data Recipient may share Data, with or without modifications, so
long as the Data Recipient makes available the text of this agreement
with the shared Data.

## 3. No Restrictions on Results

3.1. This agreement does not impose any restriction or obligations
with respect to the use, modification, or sharing of Results.

## 4. No Warranty; Limitation of Liability

4.1. All Data Recipients receive the Data subject to the following
terms:

THE DATA IS PROVIDED ON AN "AS IS" BASIS, WITHOUT REPRESENTATIONS,
WARRANTIES OR CONDITIONS OF ANY KIND, EITHER EXPRESS OR IMPLIED
INCLUDING, WITHOUT LIMITATION, ANY WARRANTIES OR CONDITIONS OF TITLE,
NON-INFRINGEMENT, MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.

NO DATA PROVIDER SHALL HAVE ANY LIABILITY FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING
WITHOUT LIMITATION LOST PROFITS), HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE DATA OR RESULTS,
EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGES.

## 5. Definitions

5.1. "Data" means the material received by a Data Recipient under
this agreement.

5.2. "Data Provider" means any person who is the source of Data
provided under this agreement and in reliance on a Data Recipient's
agreement to its terms.

5.3. "Data Recipient" means any person who receives Data directly
or indirectly from a Data Provider and agrees to the terms of this
agreement.

5.4. "Results" means any outcome obtained by computational analysis
of Data, including for example machine learning models and models'
insights.
````

### Blue Oak Model License 1.0.0

`minimatch`:

````text
# Blue Oak Model License

Version 1.0.0

## Purpose

This license gives everyone as much permission to work with
this software as possible, while protecting contributors
from liability.

## Acceptance

In order to receive this license, you must agree to its
rules. The rules of this license are both obligations
under that agreement and conditions to your license.
You must not do anything with this software that triggers
a rule that you cannot or will not follow.

## Copyright

Each contributor licenses you to do everything with this
software that would otherwise infringe that contributor's
copyright in it.

## Notices

You must ensure that everyone who gets a copy of
any part of this software from you, with or without
changes, also gets the text of this license or a link to
<https://blueoakcouncil.org/license/1.0.0>.

## Excuse

If anyone notifies you in writing that you have not
complied with [Notices](#notices), you can keep your
license by taking all practical steps to comply within 30
days after the notice. If you do not do so, your license
ends immediately.

## Patent

Each contributor licenses you to do everything with this
software that would otherwise infringe any patent claims
they can license or become able to license.

## Reliability

No contributor can revoke this license.

## No Liability

**_As far as the law allows, this software comes as is,
without any warranty or condition, and no contributor
will be liable to anyone for any damages related to this
software or this license, under any kind of legal claim._**
````
