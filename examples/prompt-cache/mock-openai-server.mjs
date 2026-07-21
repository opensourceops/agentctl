import { createServer } from "node:http";

let requestCount = 0;

async function readJson(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

const server = createServer(async (request, response) => {
  if (request.method !== "POST" || request.url !== "/responses") {
    response.statusCode = 404;
    response.end("not found");
    return;
  }

  const body = await readJson(request);
  requestCount += 1;
  const text = `prompt-cache response ${requestCount}`;
  const cachedTokens = requestCount === 1 ? 0 : 850;

  console.log(
    JSON.stringify(
      {
        request: requestCount,
        prompt_cache_key: body.prompt_cache_key,
        prompt_cache_retention: body.prompt_cache_retention,
      },
      null,
      2,
    ),
  );

  response.statusCode = 200;
  response.setHeader("content-type", "application/json");
  response.end(
    JSON.stringify({
      id: `resp_${requestCount}`,
      object: "response",
      created_at: 0,
      status: "completed",
      model: "gpt-5-mini",
      usage: {
        input_tokens: 1300,
        input_tokens_details: { cached_tokens: cachedTokens },
        output_tokens: 60,
      },
      output_text: "",
      output: [
        {
          id: `msg_${requestCount}`,
          type: "message",
          role: "assistant",
          status: "completed",
          content: [{ type: "output_text", text, annotations: [] }],
        },
      ],
    }),
  );
});

server.listen(4031, "127.0.0.1", () => {
  console.log("mock OpenAI server listening on http://127.0.0.1:4031");
});
