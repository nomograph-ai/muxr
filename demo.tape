# muxr demo
# All paths and session names are generic. No real context.
# Requires: demo config at /tmp/muxr-demo/config.toml
# Setup: mkdir -p /tmp/muxr-demo/{work,personal} && cp demo-config.toml /tmp/muxr-demo/config.toml

Output demo.gif
Set Shell "zsh"
Set FontSize 15
Set Width 960
Set Height 540
Set Theme "GitHub Dark"
Set TypingSpeed 50ms
Set Padding 20

# List sessions (empty)
Type "muxr ls"
Enter
Sleep 1.5s

# Open a work session
Type "muxr work api"
Enter
Sleep 2s
Ctrl+C
Sleep 500ms

# Open another
Type "muxr work auth"
Enter
Sleep 2s
Ctrl+C
Sleep 500ms

# Open personal
Type "muxr personal blog"
Enter
Sleep 2s
Ctrl+C
Sleep 500ms

# List sessions (three running)
Type "muxr ls"
Enter
Sleep 2.5s

# Save state
Type "muxr save"
Enter
Sleep 1.5s

# TUI switcher
Type "muxr switch"
Enter
Sleep 1.5s
Type "j"
Sleep 400ms
Type "j"
Sleep 400ms
Type "k"
Sleep 400ms
Type "q"
Sleep 1s
