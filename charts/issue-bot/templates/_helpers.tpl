{{- define "serviceAccount.name" -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Issue Bot Name
*/}}
{{- define "issueBot.name" -}}
{{ $name := default .Chart.Name .Values.issueBot.nameOverride }}
{{- printf "%s-%s" $name "issue-bot" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name for the Issue Bot.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "issueBot.fullname" -}}
{{- if .Values.issueBot.fullnameOverride }}
{{- .Values.issueBot.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.issueBot.nameOverride }}
{{- $name := printf "%s-%s" $name "issue-bot" }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Issue Bot Selector labels
*/}}
{{- define "lor_e.issueBotSelectorLabels" -}}
app.kubernetes.io/component: {{ include "issueBot.name" . }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "lor_e.labels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
helm.sh/chart: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Return the static ingress anotation
*/}}
{{- define "lor_e.issueBot.ingress.annotations" -}}
{{ .Values.issueBot.ingress.annotations | toYaml }}
{{- end -}}

{{/*
Issue Bot base url
*/}}
{{- define "lor_e.ingress.hostname" -}}
issue-bot.{{ .Values.issueBot.ingress.domain }}
{{- end }}
