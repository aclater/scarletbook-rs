Fixed
- C interface: a core `Error` with no status of its own would have reached C callers as `SB_STATUS_PANIC`. It now gets a status of its class, and a test fails until every variant has its own. No error that exists today changes status or message (#308, #311).
