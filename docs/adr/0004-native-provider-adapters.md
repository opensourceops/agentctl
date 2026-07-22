# ADR 0004: Native provider adapters behind neutral contracts

Status: accepted, 2026-07-22.

OpenAI Responses, Azure OpenAI Responses, Anthropic Messages, Google Gemini generateContent, and a scripted fake implement one provider-neutral internal interface. Capabilities are negotiated before execution; provider SDK/HTTP shapes never enter durable core state.

“OpenAI-compatible” shims are rejected as a support claim because they hide native tool, continuation, reasoning, error, and usage differences. Every provider requires mock protocol coverage; live credentials are optional evidence only.
