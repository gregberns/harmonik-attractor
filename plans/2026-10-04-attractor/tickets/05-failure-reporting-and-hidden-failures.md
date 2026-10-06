# 05: Clear run-ending errors and the two failure-hiding fixes

**Repo:** `~/github/harmonik-attractor` (Rust workspace, binary `pas`). Not harmonik-v3.

**Verify:** `cargo test --workspace` in the repo. Every check below runs with the shell-script fakes in temporary git repos; no real agent, API key or subscription.

**What to build:** Never hide a failure; keep today's behaviour otherwise (design.md §5, Q47, Q56). A reported agent error stays a routable Fail; a timeout (after `max_retries`), crash, no result or launch failure stops the run as today. Error messages change only where today's can't be understood (Q56): a timeout's error ("Command timed out after Nms") gains the node id, attempt and transcript and stderr paths; a crash's error, whose stderr now goes to a file (02b), gains the last stderr lines and the stderr path. Other messages are unchanged. Fix (a): when a node has outgoing edges and none matches, the run stops with an error naming the node, its outcome and the conditions, instead of following the first edge. For a non-FAIL outcome this deliberately diverges from the spec, which ends the run normally (spec 390-392): a graph whose edges match nothing is a mistake to report, not a success. Fix (b): a node that returns Retry on its last attempt stops the run with "still retrying after N attempts". Tool nodes are unchanged.

**Blocked by:** 02b

**Status:** ready-for-agent

- [ ] Hang with a short timeout and `max_retries=1`: two attempts, then the run fails with an error containing the node id, the attempt number and the transcript and stderr paths
- [ ] Crash: the error contains the exit code, the fake's last stderr line and the stderr path
- [ ] A node whose fake reports failure, with only an `outcome=success` edge, stops the run with the fix (a) error (previously it followed the edge)
- [ ] A Success with only non-matching conditional edges also stops the run (previously it followed the first edge)
- [ ] A unit test covers edge selection returning no edge when every edge is conditional and none matches
- [ ] A handler double that returns Retry on every attempt stops the run with the fix (b) error after `max_retries`
- [ ] A tool node with a non-zero exit still routes on an `outcome=fail` edge; a tool timeout still stops the run
- [ ] No existing test asserts the first-edge fallback directly; any test that depended on it shows up when the suite runs and is updated; the rest of the suite is green
