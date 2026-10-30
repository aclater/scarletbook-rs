/* SPDX-License-Identifier: MIT OR Apache-2.0 */
/*
 * Smoke test of the C ABI: opens an image, prints its areas and tracks, and decodes every
 * track that has audio. Exits 0 on success, 1 on an unexpected error.
 *
 *   cc -std=c99 -Wall -Wextra -Werror -pedantic -Iinclude examples/smoke.c \
 *      ../../target/release/libscarletbook_ffi.a -lpthread -ldl -lm -o smoke
 */
#include <stdio.h>
#include <stdlib.h>
#include "scarletbook.h"

static int fail(const char *what, enum SbStatus s) {
    fprintf(stderr, "%s: %s (%s)\n", what, sb_status_name((int32_t)s), sb_last_error_message());
    return 1;
}

int main(int argc, char **argv) {
    struct SbDisc *disc = NULL;
    struct SbDiscInfo info;
    enum SbStatus s;
    uint32_t a;
    char title[256];
    size_t len = 0;

    if (argc != 2) {
        fprintf(stderr, "usage: smoke IMAGE\n");
        return 2;
    }
    printf("scarletbook %s\n", sb_version());
    s = sb_disc_open_file(argv[1], &disc);
    if (s != SB_STATUS_OK) {
        /* A rejected image is a valid outcome, not a failure of the library. */
        printf("rejected: %s\n", sb_status_name((int32_t)s));
        return s < SB_STATUS_NO_AUDIO_AREAS ? 1 : 0;
    }
    if ((s = sb_disc_info(disc, &info)) != SB_STATUS_OK) return fail("sb_disc_info", s);
    printf("sector size %u, %llu sectors, %u areas, %u warnings\n", info.sector_size,
           (unsigned long long)info.sectors, info.area_count, info.warning_count);
    s = sb_disc_string(disc, SB_DISC_STRING_ALBUM_TITLE, title, sizeof title, &len);
    if (s == SB_STATUS_OK) printf("album title: %s\n", title);

    for (a = 0; a < info.area_count; a++) {
        struct SbAreaInfo area;
        uint32_t t;
        if ((s = sb_disc_area(disc, a, &area)) != SB_STATUS_OK) return fail("sb_disc_area", s);
        printf("area slot %u: %u channels, %u tracks\n", area.slot, area.channel_count, area.track_count);
        for (t = 1; t <= area.track_count; t++) {
            struct SbTrackAudio *audio = NULL;
            struct SbFrameInfo fi;
            uint8_t *buf;
            size_t cap;
            uint64_t frames = 0, missing = 0, invalid = 0;
            s = sb_track_audio_open(disc, area.slot, t, &audio);
            if (s != SB_STATUS_OK) {
                printf("  track %u: no audio (%s)\n", t, sb_status_name((int32_t)s));
                continue;
            }
            cap = (size_t)sb_track_audio_channels(audio) * 4704;
            buf = malloc(cap);
            if (buf == NULL) return 1;
            while ((s = sb_track_audio_read(audio, disc, buf, cap, &fi)) == SB_STATUS_OK) frames++;
            free(buf);
            if (s != SB_STATUS_END) return fail("sb_track_audio_read", s);
            sb_track_audio_counts(audio, &missing, &invalid);
            sb_track_audio_free(audio);
            printf("  track %u: %llu frames, %llu missing, %llu invalid\n", t, (unsigned long long)frames,
                   (unsigned long long)missing, (unsigned long long)invalid);
        }
    }
    sb_disc_free(disc);
    return 0;
}
