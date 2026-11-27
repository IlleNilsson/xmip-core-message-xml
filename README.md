# xmip-core-message-xml

XML shape: one well-formed document as one part, the root element's expanded name as the announced type. A technology of [xmip-core-message](https://github.com/IlleNilsson/xmip-core-message).

The document is walked by the estate's one XML reader, `codec::xml::Reader`,
which says the byte where bytes stop being one document; until 2026-11-13 the
shape walked them itself.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
