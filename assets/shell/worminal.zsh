# Optional Zsh integration. In ~/.zshrc:
# [[ -n ${WORMINAL_DAEMON-} ]] && eval "$("$WORMINAL_DAEMON" api help shell-zsh)"
# Reports OSC 133 command boundaries and exit status; directory comes from the PTY process.
if [[ -o interactive && -n ${WORMINAL_DAEMON-} && -z ${_worminal_integrated-} ]]; then
    _worminal_integrated=1
    _worminal_precmd() {
        local worminal_command_status=$?
        if [[ -n ${_worminal_prompt_seen-} ]]; then printf '\033]133;D;%d\007' "$worminal_command_status"; fi
        _worminal_prompt_seen=1
        printf '\033]133;A\007'
        return 0
    }
    _worminal_preexec() { printf '\033]133;C\007'; }
    precmd_functions=(_worminal_precmd ${precmd_functions[@]})
    preexec_functions+=(_worminal_preexec)
    PS1+=$'%{\e]133;B\a%}'
fi
