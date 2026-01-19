# Roadmap

Mark all activities with a ✅ once complete.

## Phase 1 (io_uring)
- Replace recvmmsg with io_uring for:
    - 80-95% syscall reduction
    - Zero-copy operations
    - 50-150% throughput improvement

## Phase 2 Security Hardening
- Rate limiting per client IP. 
- Connection tracking and DoS prevention. 
- Additional NIST control implementations. 