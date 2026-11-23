# Native host fixtures

The canonical messages and views of protocol 1. The browser repo, devlanes-run/browser, copies them into `packages/protocol/test/fixtures/` and tests its parsers on them. Change them here first, then copy them there.

`view-*.json` are composed from the daemon replies in `../daemon/`. `messages.json` holds the requests the host reads, and the replies and events it sends. The crate's tests check both against the host's code.
