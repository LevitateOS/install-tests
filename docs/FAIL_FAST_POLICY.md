# Fail Fast Policy

## The Problem We Had

### Long Timeouts Are Disrespectful

When a command fails in 2 seconds but the test waits 120 seconds "just in case," that's **118 seconds of the developer's life wasted per failure**.

Run the test 10 times during debugging? That's **20 minutes of staring at nothing**.

The developer knows something is wrong at second 2. The test knows something is wrong at second 2 (errors are streaming by). But the code sits there, waiting, hoping, praying that maybe the timeout will magically fix things.

It won't. It never does.

### Increasing Timeouts to "Fix" Errors Is Even Worse

This is the debugging equivalent of closing your eyes and hoping the problem goes away.

When a test fails, increasing the timeout says:
- "I don't understand why this failed"
- "I'm too lazy to investigate"
- "Maybe if I wait longer, the universe will fix it for me"

**The error message tells you exactly what's wrong.** A missing dependency doesn't need more time - it needs the dependency.

Increasing timeouts:
1. Wastes even MORE of the developer's time on subsequent runs
2. Hides the real problem behind a wall of waiting
3. Makes the developer think "maybe it's just slow" instead of "something is broken"
4. Compounds the disrespect with every single test run

### The Real Cost

A developer's time is not free. Every minute spent waiting for a timeout that will never succeed is:
- Money wasted (API tokens, compute, salary)
- Focus destroyed (context switching while waiting)
- Trust eroded (the developer loses faith in the tooling)
- Emotional harm (frustration, feeling disrespected)

## The Policy Now

### 1. Fail Immediately on Error Patterns

When we see `Kernel panic`, `Segmentation fault`, `FATAL:` - **stop immediately**. Don't wait. Don't hope. Return failure NOW.

```rust
const FATAL_ERROR_PATTERNS: &[&str] = &[
    "FATAL:",
    "Kernel panic",
    "Segmentation fault",
    "core dumped",
    // etc.
];
```

### 2. Timeouts Are Maximums, Not Goals

A 30-second timeout means "if this takes more than 30 seconds, something is catastrophically wrong." It does NOT mean "wait 30 seconds before checking if it worked."

### 3. When Something Fails, Investigate - Don't Extend

If a test fails at 29 seconds with a timeout of 30 seconds:
- **WRONG**: Increase timeout to 60 seconds
- **RIGHT**: Ask "why did it take 29 seconds? What's slow? What's broken?"

### 4. Kill QEMU Immediately on Failure

Don't let a failed test keep running. The moment a step fails:
1. Print the error
2. Kill the VM
3. Exit with failure
4. Clean up resources

No "let's see what else fails." No "maybe we can recover." Dead. Immediately.

## Implementation

See `src/qemu/console.rs`:
- `FATAL_ERROR_PATTERNS` - patterns that trigger immediate failure
- `exec()` - checks for fatal patterns on every line of output
- Returns immediately with `aborted_on_error: true` when pattern detected

See `src/main.rs`:
- On step failure: `child.kill()`, `bail!("Installation tests failed")`

## Remember

**Your time matters. Tests that waste it are broken tests.**
