# Optional Bash 4.4+ integration. In ~/.bashrc:
# [[ -n ${WORMINAL_DAEMON-} ]] && eval "$("$WORMINAL_DAEMON" api help shell-bash)"
# Reports OSC 133 command boundaries and exit status; directory comes from the PTY process.
if [[ $- == *i* && -n ${WORMINAL_DAEMON-} && -z ${_worminal_integrated-} ]]; then
    _worminal_integrated=1
    _worminal_prompt() {
        local worminal_command_status=$?
        if [[ -n ${_worminal_prompt_seen-} ]]; then printf '\033]133;D;%d\007' "$worminal_command_status"; fi
        _worminal_prompt_seen=1
        printf '\033]133;A\007'
        return "$worminal_command_status"
    }
    PROMPT_COMMAND=(_worminal_prompt "${PROMPT_COMMAND[@]}")
    PS0=$'\e]133;C\a'"${PS0-}"
    PS1+=$'\[\e]133;B\a\]'
fi
