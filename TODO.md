# Priorities

- [ ] deploy issue-bot to k8s manually
  - [x] for now deploy w/out publishing comments
  - [ ] instead publish to slack channel for reviewing and finetuning

- [x] on open issue event, do retrieval and suggest 3/5 closest issues / prs
- [x] on open discussion event, do retrieval and suggest 3/5 closest issues / prs

- [x] do the following tasks for GH webhooks
- [x] do the following tasks for HF webhooks
  - [x] store issue / PR with its affiliated comments
  - [x] on new comment or description edit, update values in db and (re)compute embedding
  - [x] on deletion, delete comment or issue/pr and update or remove embedding

- [x] script or endpoint to index existing issues for a repo
  - [ ] if github app, index repo on app install

- [ ] move to github app
- [x] create hf bot user

- [x] make bot message configurable (from env / config file)

- [ ] make sure issue is not re-indexed with the bot's messages
- [x] make sure discussion is not re-indexed with the bot's messages

- [x] fix github issue link (currently `api.github.com` instead of the regular github UI url)

- [x] fix: delete associated comments, reviews & review comments

## Infra resilience tasks

- [ ] helm chart for the issue bot
- [ ] terraform code to deploy the cloudnativepg operator and the actual cluster's crd
- [ ] secret mgmt

# Ideas

- [ ] bot command to ask bot to suggest new similar issues / update previous comment
