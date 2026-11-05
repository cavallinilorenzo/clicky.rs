# Clicky

A macOS menu bar app that plays mechanical keyboard sounds on every key press, so that any keyboard sounds like a mechanical one.

## Language

### Product

**Clicky**:
The app itself. Always capitalised, and never used for a switch type.
_Avoid_: Clicky.rs (the repository name), the tool

**Clicky switch**:
A switch type whose keystroke has a sharp audible click (e.g. Cherry MX Blue, Kailh Box Navy). In the menu its group is labelled "Clicky".
_Avoid_: clicky (alone, when the app could be meant)

### Sounds

**Soundpack**:
A named set of recordings of one keyboard, giving a press sound (and optionally a release sound) for each key.
_Avoid_: pack theme, sound set, profile

**Switch type**:
The group a soundpack belongs to: Linear, Tactile or Clicky switch.
_Avoid_: category, family

**Press sound**:
The sound of a key going down.
_Avoid_: keydown sound, downstroke

**Release sound**:
The sound of a key coming back up. Only some soundpacks have release sounds.
_Avoid_: keyup sound, upstroke

**Voice**:
One press or release sound currently playing. Several voices can sound at the same time.
_Avoid_: channel, track

### Output

**Warm output**:
The state in which the audio output is running and a key press is heard at once.
_Avoid_: active, awake

**Cold output**:
The state in which the audio output is stopped and holds no device, as when the user is not typing.
_Avoid_: idle, sleeping (these mean the Mac's own states)

**Hold window**:
How long the output stays warm after the last key press before it goes cold.
_Avoid_: idle timeout, keep-alive

**Pre-roll**:
Silence played after a cold start and before the first voice, so that a slow device (such as Bluetooth headphones) is ready to make it heard.
_Avoid_: warm-up, padding
