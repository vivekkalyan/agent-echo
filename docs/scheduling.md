# Schedule hourly collection

Install `agent-echo` and test a manual collection before enabling a scheduler. Keep the configuration and state directory outside your synced vault. Assign a different producer identity to each machine.

## Linux with a user systemd service

Copy `examples/systemd/agent-echo.service` and `agent-echo.timer` into `~/.config/systemd/user/`. Adjust the binary path if needed.

```sh
systemctl --user daemon-reload
systemctl --user enable --now agent-echo.timer
systemctl --user start agent-echo.service
journalctl --user -u agent-echo.service
```

The timer runs after the user service manager starts and hourly thereafter. A sleeping or stopped machine cannot collect. Containers without systemd can call the same command from their existing supervisor at startup and every hour.

To stop collection, disable the timer.

```sh
systemctl --user disable --now agent-echo.timer
```

## macOS with launchd

Copy `examples/launchd/com.agent-echo.collect.plist` to `~/Library/LaunchAgents/`. Replace every `/Users/example` path with your actual home directory. Launchd does not expand `~` in program arguments. Create the configured log directory before loading the job.

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.agent-echo.collect.plist
launchctl kickstart gui/$(id -u)/com.agent-echo.collect
```

The job runs at load and every 3,600 seconds while the login session is active. Review the configured stdout and stderr files for failures. Desktop Obsidian Sync still needs the app to run before uploads happen.

To stop collection, unload the job.

```sh
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.agent-echo.collect.plist
```
