"""Named harness, model, and thinking combinations."""

from dataclasses import dataclass


THINKING_LEVELS = ("off", "minimal", "low", "medium", "high", "xhigh", "max")


@dataclass(frozen=True)
class Preset:
    name: str
    harness: str
    model: str
    thinking: str = ""


def load_presets(config):
    values = config.get("presets", [])
    if not isinstance(values, list):
        raise ValueError("presets must contain a list of named combinations")
    presets, names = [], set()
    for value in values:
        if not isinstance(value, dict) or any(not isinstance(value.get(key), str) or not value[key].strip() for key in ("name", "harness", "model")):
            raise ValueError("Each preset needs a name, harness, and native model ID")
        name, harness, model = (value[key].strip() for key in ("name", "harness", "model"))
        thinking = value.get("thinking", "")
        if thinking not in ("",) + THINKING_LEVELS:
            raise ValueError("Preset thinking must be off, minimal, low, medium, high, xhigh, or max")
        if name in names:
            raise ValueError("Preset names must be unique: " + name)
        names.add(name)
        presets.append(Preset(name, harness, model, thinking))
    return tuple(presets)
