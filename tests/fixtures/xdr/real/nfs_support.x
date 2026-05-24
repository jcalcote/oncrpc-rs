const REMOTE_HANDLE_SIZE = 32;

typedef opaque remote_handle_t<REMOTE_HANDLE_SIZE>;

program NFS_SUPPORT_PROGRAM {
    version NFS_SUPPORT_V1 {
        void NFS_PING(void) = 0;
    } = 1;
} = 300001;
