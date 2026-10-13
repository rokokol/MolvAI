# Model licenses

The weights of the recognition models and of the language models are not part of the MolvAI repository, and the license of the project does not cover them. The repository holds only the code that downloads and loads these weights. So the MIT license in `LICENSE` applies to the code and to nothing else

> [!IMPORTANT]
> The author of the weights sets the terms of use, not MolvAI. Before commercial use, check the license of the exact model with the links below

## Whisper (speech recognition)

OpenAI released the Whisper model under the MIT license, both the weights and the code of the reference implementation. The weights converted to the `ggml` format, which `whisper.cpp` uses, have the same MIT license and are on Hugging Face

| What | License | Where to get it |
| --- | --- | --- |
| `ggml-*.bin` weights for whisper.cpp | MIT | [huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) |
| Original OpenAI Whisper weights | MIT | [github.com/openai/whisper](https://github.com/openai/whisper) |
| Code of the whisper.cpp engine | MIT | [github.com/ggml-org/whisper.cpp](https://github.com/ggml-org/whisper.cpp) |

By default MolvAI downloads `ggml-small.bin`. You change the model with the `stt.model` setting and the file path with the `stt.model_path` setting

## Post-processing language models

Post-processing of the text by a language model is off by default and is not needed for operation: the dictionary and the rules work without it. When it is on, MolvAI calls an OpenAI-compatible API at the address from the `llm.base_url` setting. By default this is a local Ollama at `http://localhost:11434/v1`

In this case the license of the model is the business of the person who runs it. `qwen3.5` has Apache 2.0, `llama` has its own Meta license, and cloud models work under a contract with the provider. MolvAI ships and downloads none of them

## Check before a release

The downloaded weights are checked against SHA-256 (criterion A-08). The checksums are next to the description of the model in the loader code. The second run reuses the weights cache, and no repeated download happens

The licenses of the code dependencies are a separate matter. They are in [THIRD-PARTY.md](../THIRD-PARTY.md) and the `just deny` gate checks them
