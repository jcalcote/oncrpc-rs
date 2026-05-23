const FILE_HANDLE_LEN = 64;

typedef unsigned hyper pdx_trc_id_t;
typedef opaque pdx_file_handle_t<FILE_HANDLE_LEN>;
typedef string pdx_path_t<>;

struct pdx_time_t {
    unsigned int seconds;
    unsigned int nseconds;
};

enum pdx_status_t {
    PDXERR_OK = 0,
    PDXERR_NOENT = 2,
    PDXERR_STALE = 116,
    PDXERR_FAILED = 1001
};
