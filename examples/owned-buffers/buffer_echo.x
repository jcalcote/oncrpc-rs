const ECHO_MAX = 64;
typedef opaque echo_data<ECHO_MAX>;

program BUFFER_ECHO {
    version BUFFER_ECHO_V1 {
        echo_data ECHO(echo_data) = 1;
    } = 1;
} = 0x31230002;
