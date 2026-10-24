/* NFSv3 I/O subset: RFC 1813 sections 2.2, 2.5, 3.3.6, 3.3.7 and 3.3.21.
 * https://www.rfc-editor.org/rfc/rfc1813.html
 * This fixture retains the protocol layouts and RPC numbers; it is not a
 * complete NFS service implementation.
 */
const NFS3_FHSIZE = 64;
const NFS3_WRITEVERFSIZE = 8;

typedef unsigned int uint32;
typedef unsigned hyper uint64;
typedef uint64 fileid3;
typedef uint32 uid3;
typedef uint32 gid3;
typedef uint64 size3;
typedef uint64 offset3;
typedef uint32 mode3;
typedef uint32 count3;
struct nfs_fh3 {
    opaque data<NFS3_FHSIZE>;
};
typedef opaque writeverf3[NFS3_WRITEVERFSIZE];

enum nfsstat3 {
    NFS3_OK = 0,
    NFS3ERR_PERM = 1,
    NFS3ERR_NOENT = 2,
    NFS3ERR_IO = 5,
    NFS3ERR_NXIO = 6,
    NFS3ERR_ACCES = 13,
    NFS3ERR_EXIST = 17,
    NFS3ERR_XDEV = 18,
    NFS3ERR_NODEV = 19,
    NFS3ERR_NOTDIR = 20,
    NFS3ERR_ISDIR = 21,
    NFS3ERR_INVAL = 22,
    NFS3ERR_FBIG = 27,
    NFS3ERR_NOSPC = 28,
    NFS3ERR_ROFS = 30,
    NFS3ERR_MLINK = 31,
    NFS3ERR_NAMETOOLONG = 63,
    NFS3ERR_NOTEMPTY = 66,
    NFS3ERR_DQUOT = 69,
    NFS3ERR_STALE = 70,
    NFS3ERR_REMOTE = 71,
    NFS3ERR_BADHANDLE = 10001,
    NFS3ERR_NOT_SYNC = 10002,
    NFS3ERR_BAD_COOKIE = 10003,
    NFS3ERR_NOTSUPP = 10004,
    NFS3ERR_TOOSMALL = 10005,
    NFS3ERR_SERVERFAULT = 10006,
    NFS3ERR_BADTYPE = 10007,
    NFS3ERR_JUKEBOX = 10008
};

enum ftype3 {
    NF3REG = 1,
    NF3DIR = 2,
    NF3BLK = 3,
    NF3CHR = 4,
    NF3LNK = 5,
    NF3SOCK = 6,
    NF3FIFO = 7
};

struct specdata3 {
    uint32 specdata1;
    uint32 specdata2;
};

struct nfstime3 {
    uint32 seconds;
    uint32 nseconds;
};

struct fattr3 {
    ftype3 type;
    mode3 mode;
    uint32 nlink;
    uid3 uid;
    gid3 gid;
    size3 size;
    size3 used;
    specdata3 rdev;
    uint64 fsid;
    fileid3 fileid;
    nfstime3 atime;
    nfstime3 mtime;
    nfstime3 ctime;
};

union post_op_attr switch (bool attributes_follow) {
case TRUE:
    fattr3 attributes;
case FALSE:
    void;
};

struct wcc_attr {
    size3 size;
    nfstime3 mtime;
    nfstime3 ctime;
};

union pre_op_attr switch (bool attributes_follow) {
case TRUE:
    wcc_attr attributes;
case FALSE:
    void;
};

struct wcc_data {
    pre_op_attr before;
    post_op_attr after;
};

struct READ3args {
    nfs_fh3 file;
    offset3 offset;
    count3 count;
};

struct READ3resok {
    post_op_attr file_attributes;
    count3 count;
    bool eof;
    opaque data<>;
};

struct READ3resfail {
    post_op_attr file_attributes;
};

union READ3res switch (nfsstat3 status) {
case NFS3_OK:
    READ3resok resok;
default:
    READ3resfail resfail;
};

enum stable_how {
    UNSTABLE = 0,
    DATA_SYNC = 1,
    FILE_SYNC = 2
};

struct WRITE3args {
    nfs_fh3 file;
    offset3 offset;
    count3 count;
    stable_how stable;
    opaque data<>;
};

struct WRITE3resok {
    wcc_data file_wcc;
    count3 count;
    stable_how committed;
    writeverf3 verf;
};

struct WRITE3resfail {
    wcc_data file_wcc;
};

union WRITE3res switch (nfsstat3 status) {
case NFS3_OK:
    WRITE3resok resok;
default:
    WRITE3resfail resfail;
};

struct COMMIT3args {
    nfs_fh3 file;
    offset3 offset;
    count3 count;
};

struct COMMIT3resok {
    wcc_data file_wcc;
    writeverf3 verf;
};

struct COMMIT3resfail {
    wcc_data file_wcc;
};

union COMMIT3res switch (nfsstat3 status) {
case NFS3_OK:
    COMMIT3resok resok;
default:
    COMMIT3resfail resfail;
};

program NFS_PROGRAM {
    version NFS_V3 {
        READ3res NFSPROC3_READ(READ3args) = 6;
        WRITE3res NFSPROC3_WRITE(WRITE3args) = 7;
        COMMIT3res NFSPROC3_COMMIT(COMMIT3args) = 21;
    } = 3;
} = 100003;
