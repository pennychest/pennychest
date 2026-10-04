# AI integration

PennyChest can use an LLM to suggest categories for uncategorised transactions and propose new rules. It's optional.

Configure it in the app under **Settings → AI Integration**.

=== "Ollama (self-hosted, free)"

    The development Compose stack includes an `ollama` service. Pull the recommended model after starting the services:

    ```bash
    docker exec pc-ollama-1 ollama pull qwen3:4b
    ```

    Then switch the provider to **Ollama** and set:

    - **URL:** `http://ollama:11434`
    - **Model:** `qwen3:4b`

=== "Claude (Anthropic API)"

    Get an API key from [console.anthropic.com](https://console.anthropic.com), keep the provider as **Claude**, and paste your key.

    !!! note
        The Anthropic API is billed per token and is separate from a claude.ai subscription.
