# Priorities

- [ ] on open PR/issue event, do retrieval and suggest 3/5 closest issues / prs
- [ ] on comment events, if a PR has no comment from bot, do suggestion

- [ ] do the following tasks for GH webhooks
- [ ] do the following tasks for HF webhooks
  - [ ] store issue / PR with its affiliated comments
    - id should be: {owner}/{repo_name}/{number}
  - [ ] on new comment or description edit, update values in db and (re)compute embedding
  - [ ] on deletion, delete comment or issue/pr and update or remove embedding

# Ideas

- [ ] bot command to ask bot to suggest new similar issues / update previous comment
