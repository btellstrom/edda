{{/* The name things are called by: the release's, unless it says edda already. */}}
{{- define "edda.fullname" -}}
{{- if contains "edda" .Release.Name -}}
{{- .Release.Name | trunc 50 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-edda" .Release.Name | trunc 50 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "edda.labels" -}}
app.kubernetes.io/name: edda
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "edda.selector" -}}
app.kubernetes.io/name: edda
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .service }}
{{- end -}}

{{- define "edda.image" -}}
{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}
{{- end -}}

{{/* A service's address inside the cluster. */}}
{{- define "edda.url" -}}
http://{{ include "edda.fullname" .root }}-{{ .service }}:{{ (index .root.Values.services .service).port }}
{{- end -}}

{{/* Each service's command, and the name of its database in the Secret. */}}
{{- define "edda.command" -}}
{{- $commands := dict "auth" (list "auth-service") "characters" (list "character-service") "combat" (list "combat-service") "monsters" (list "monster-service" "serve") -}}
{{- toJson (index $commands .) -}}
{{- end -}}

{{- define "edda.database" -}}
{{- $databases := dict "auth" "auth" "characters" "characters" "combat" "combats" "monsters" "monsters" -}}
{{- index $databases . -}}
{{- end -}}

{{/* What every service is told, whichever it is. */}}
{{- define "edda.env" -}}
{{- $root := .root -}}
{{- $database := include "edda.database" .service -}}
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
      name: {{ $root.Values.database.secret }}
      key: {{ $database }}
{{- if $root.Values.database.direct }}
- name: DATABASE_DIRECT_URL
  valueFrom:
    secretKeyRef:
      name: {{ $root.Values.database.secret }}
      key: {{ $database }}-direct
{{- end }}
- name: BIND_ADDR
  value: "0.0.0.0:{{ (index $root.Values.services .service).port }}"
- name: LIMITS
  value: {{ ternary "on" "off" $root.Values.limits.enabled | quote }}
{{- range $name, $value := $root.Values.limits.env }}
- name: {{ $name }}
  value: {{ $value | quote }}
{{- end }}
- name: TRUST_PROXY
  value: {{ ternary "1" "0" $root.Values.trustProxy | quote }}
- name: LOG_FORMAT
  value: {{ $root.Values.logFormat | quote }}
{{- with $root.Values.otlpEndpoint }}
- name: OTEL_EXPORTER_OTLP_ENDPOINT
  value: {{ . | quote }}
{{- end }}
{{- with $root.Values.corsOrigins }}
- name: CORS_ORIGINS
  value: {{ . | quote }}
{{- end }}
{{- if $root.Values.metrics.enabled }}
- name: METRICS_ADDR
  value: "0.0.0.0:{{ $root.Values.metrics.port }}"
{{- end }}
{{- with $root.Values.admin.tokenSecret }}
- name: ADMIN_ADDR
  value: "0.0.0.0:{{ $root.Values.admin.port }}"
- name: ADMIN_TOKEN
  valueFrom:
    secretKeyRef:
      name: {{ . }}
      key: token
{{- end }}
{{- if ne .service "auth" }}
- name: AUTH_SERVICE_URL
  value: {{ include "edda.url" (dict "root" $root "service" "auth") | quote }}
{{- end }}
{{- end -}}
