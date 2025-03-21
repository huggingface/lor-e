# Priorities

- [x] on open PR/issue event, do retrieval and suggest 3/5 closest issues / prs

- [x] do the following tasks for GH webhooks
- [ ] do the following tasks for HF webhooks
  - [x] store issue / PR with its affiliated comments
  - [x] on new comment or description edit, update values in db and (re)compute embedding
  - [x] on deletion, delete comment or issue/pr and update or remove embedding

- [ ] script to index existing issues for a repo

- [ ] move to github app

- [ ] make bot message configurable (from env / config file)

- [ ] make sure issue is not re-indexed with the bot's messages

- [x] fix github issue link (currently `api.github.com` instead of the regular github UI url)

# Ideas

- [ ] bot command to ask bot to suggest new similar issues / update previous comment
