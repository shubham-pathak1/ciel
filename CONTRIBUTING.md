# Contributing to Ciel

Thanks for taking the time to contribute.

Ciel is a solo-maintained project, so the biggest help is small, focused changes that are easy to review, test, and ship safely.

## Before You Start

- Check existing issues and pull requests before starting work.
- Prefer one fix or feature per pull request.
- If the change affects download correctness, resume behavior, or file safety, include clear reproduction steps.

## Development Setup

```bash
npm install
npm run tauri dev
```

Optional:

- Copy `src-tauri/.cargo/config.toml.example` to `src-tauri/.cargo/config.toml` if you want a custom Rust target directory.

## Project Priorities

When in doubt, optimize for these in order:

1. Correctness
2. Reliability
3. Clear UX
4. Performance
5. New features

For Ciel, a smaller fix that makes downloads safer is usually better than a larger feature that adds maintenance cost.

## Contribution Guidelines

- Keep changes scoped and intentional.
- Avoid unrelated refactors in feature or bugfix PRs.
- Preserve existing product direction: lean, local-first, sidecar-free where possible.
- Prefer readable, maintainable code over clever code.
- Add or update logs only when they improve diagnosis of real issues.
- Surface user-facing failures clearly instead of silently swallowing them.

## Frontend Notes

- Match the existing visual language unless the PR is explicitly a UI redesign.
- Keep states honest. Do not show progress or resume behavior that the backend cannot actually guarantee.
- Avoid introducing heavy dependencies for simple UI changes.

## Backend Notes

- Be careful with pause/resume semantics, chunk persistence, and fallback behavior.
- Network changes should be tested against both:
  - a good/resumable source
  - a bad/non-resumable or throttled source
- Never trade correctness for synthetic speed.

## Testing Expectations

For changes in download logic, try to include:

- what you changed
- how you tested it
- what kind of URL or torrent you used
- whether the scenario was fresh start, pause/resume, or restart recovery

If you could not test something, say that clearly in the PR.

## Pull Request Checklist

- Code builds locally
- Change is scoped
- Logs/errors are intentional
- User-facing behavior is clear
- README/docs updated if needed

## Issues

Bug reports are most useful when they include:

- exact steps to reproduce
- expected behavior
- actual behavior
- screenshots if UI-related
- terminal logs if backend-related

Thanks again for helping improve Ciel.
