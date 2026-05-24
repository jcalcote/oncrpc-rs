typedef string time_string<64>;

program TIME_SERVICE {
    version TIME_SERVICE_V1 {
        time_string GET_TIME(void) = 1;
    } = 1;
} = 0x31230001;
