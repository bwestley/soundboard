# Soundboard

This is a simple, easy-to-use soundboard programmed in Rust. Designed for use with [bwestley/remote-input](https://github.com/bwestley/remote-input).

## Features

* Play multiple sounds simultaneously
* Pause and resume single or all sound playback
* Output to multiple audio devices simultaneously
* Mute and unmute each output with a button press
* Fully configurable from the GUI, all settings are automatically stored in a human-readable config.toml file
* A client for [bwestley/remote-input](https://github.com/bwestley/remote-input), a rust program that sends keyboard events from a separate linux machine over the network

## QUICK START GUIDE

1. Install <https://github.com/bwestley/remote-input> on a linux device.
2. Configure the remote input server with a "config.toml" file such as the template in this manual. If a "config.toml" file cannot be found, a default matching this template will be automatically installed.
3. Start the remote input server.
4. Open settings.
5. Set the server address and API key.
6. Press "Connect" in the main window to connect to the configured remote input server.
7. Select output devices.
8. Set keybinds:
    * To clear: right click.
    * To set: left click before pressing a key on the remote. Left click again to cancel.
9. Close settings.
10. Add a new sound by filling the following fields from left to right before clicking the "Add" button.
    * Name
    * Key: see step 8
    * Volume: drag the slider or enter a number
    * Path: type in a path or click and drag a file
11. Click the large toggle switch to enable the soundboard.
12. On the remote, press the key that was selected in step 10 to play that sound!

## KEY BIND BUTTONS

Key bind buttons store a specific key to trigger behavior when that key is later pressed on that remote.
To clear: right click.
To set: left click before pressing a key on the remote. Left click again to cancel.

## SOUNDS

The output devices selected in the settings menu are listed with their volume control and mute status. A sound can be added by pressing the "Add" button on the top row of the sounds table. The fields will then be moved down into the next row. These can be edited at any time. The sound-specific volume and keybind settings take effect immediately. Press the "^" or "v" buttons to move the sounds up or down the list. The order of sounds has no effect. Press the "Remove" button to delete that sound. The indicator on the left of each sound shows if the sound is stopped, playing, or paused. When the sound ends, the indicator still shows that it is playing. Pressing the pause button (as configured in the settings menu under "Shortcuts") will pause all playing sounds. Pressing it again will play all paused sounds. Pressing the stop button (as configured...) will stop all playing and paused sounds. Pressing the modifier button (as configured...) will cause the the next button pressed to resume/pause playback instead of restarting play from the beginning of the sound. Pressing the modifier button again before pressing a sound button, or pressing a sound button will reset the modifier state.

## SETTINGS

The settings menu can be opened with the "Settings" button. When a audio device is added or remove from the computer, the audio device list can be updated with the "Reload Devices" button. Check the box next to each device audio should play from. The server address may be an IP address or DNS name followed by a port number (e.g. rpi3.lan:8650 or 192.168.1.58:8650). The associated keybind will mute and unmute that audio device. The remote input server api key should match what is in the remote server's config.toml tile. The pause, stop, and modifier keybinds can be changed in the "Shortcuts" section. See the SOUNDS section of this manual for information on shortcut function and the KEY BIND BUTTONS section for instructions on how to configure keybinds.

## REMOTE INPUT SERVER config.toml TEMPLATE

```toml
[hardware]
# The name of the keyboard device as reported by evdev:
name = "Logitech USB Keyboard"
# The status light blink duration in milliseconds
led_speed_millis = 3000
# See https://github.com/torvalds/linux/blob/master/include/uapi/linux/
# input-event-codes.h for key names.
# The escape key will ungrab and grab the input device.
escape = "KEY_SCROLLLOCK"
# The pause key will pause and unpause event transmission.
pause = "KEY_PAUSE"

[server]
# The bind address for the remote input server:
address = "0.0.0.0:8650"
# The api key (terminated by a zero byte) must be sent by
# the client when the connection is established.
api_key = "d4AXBDqWa0PQgsGVc4oKnguYA4jEfu5EM7ztD7to"
```
