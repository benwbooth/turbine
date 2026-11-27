// tessh's "…" strings: their $(…) and ${…} may hold brackets and strings
// of their own (`"${len "x"}"`), which a regular token can't follow.
// Also: redirections (2>f, 3>&1, >&-, <<<, &>), <( and >(, and heredocs
// (<<END … END), whose body is read at the end of the line that starts it.
#include "tree_sitter/parser.h"
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

enum { DQ_STRING, REDIRECTION, PROCSUB_OPEN, HEREDOC_START, HEREDOC_BODY, ERROR_SENTINEL };

// The heredocs a line has started, waiting for its end: each end word,
// with whether tabs come off (<<-).
#define MAX_PENDING 8
#define MAX_DELIM 64
typedef struct {
    int n;
    bool strip[MAX_PENDING];
    char delim[MAX_PENDING][MAX_DELIM + 1];
} State;

static void next(TSLexer *l) { l->advance(l, false); }

static bool dq_body(TSLexer *l, int depth);

// Up to the `close` of a group whose opening was just read.
static bool group(TSLexer *l, int32_t close, int depth) {
    if (depth > 64) return false;
    while (!l->eof(l)) {
        int32_t c = l->lookahead;
        if (c == close) { next(l); return true; }
        if (c == '\\') { next(l); if (!l->eof(l)) next(l); continue; }
        if (c == '"') { next(l); if (!dq_body(l, depth + 1)) return false; continue; }
        if (c == '\'') {
            next(l);
            while (!l->eof(l) && l->lookahead != '\'') next(l);
            if (l->eof(l)) return false;
            next(l);
            continue;
        }
        next(l);
        if (c == '(' && !group(l, ')', depth + 1)) return false;
        if (c == '[' && !group(l, ']', depth + 1)) return false;
        if (c == '{' && !group(l, '}', depth + 1)) return false;
    }
    return false;
}

// A string's rest, its opening `"` read: `""` empty, `"""` long.
static bool dq_body(TSLexer *l, int depth) {
    bool triple = false;
    if (l->lookahead == '"') {
        next(l);
        if (l->lookahead != '"') return true;
        next(l);
        triple = true;
    }
    while (!l->eof(l)) {
        int32_t c = l->lookahead;
        if (c == '"') {
            next(l);
            if (!triple) return true;
            if (l->lookahead == '"') {
                next(l);
                if (l->lookahead == '"') { next(l); return true; }
            }
            continue;
        }
        next(l);
        if (c == '\\') { if (!l->eof(l)) next(l); continue; }
        if (c == '$' && (l->lookahead == '(' || l->lookahead == '{')) {
            int32_t close = l->lookahead == '(' ? ')' : '}';
            next(l);
            if (!group(l, close, depth + 1)) return false;
        }
    }
    return false;
}

void *tree_sitter_tessh_external_scanner_create(void) {
    State *s = calloc(1, sizeof(State));
    return s;
}
void tree_sitter_tessh_external_scanner_destroy(void *p) { free(p); }

unsigned tree_sitter_tessh_external_scanner_serialize(void *p, char *b) {
    State *s = p;
    unsigned at = 0;
    b[at++] = (char)s->n;
    for (int i = 0; i < s->n; i++) {
        size_t len = strlen(s->delim[i]);
        if (at + 2 + len > TREE_SITTER_SERIALIZATION_BUFFER_SIZE) break;
        b[at++] = s->strip[i];
        b[at++] = (char)len;
        memcpy(b + at, s->delim[i], len);
        at += len;
    }
    return at;
}

void tree_sitter_tessh_external_scanner_deserialize(void *p, const char *b, unsigned n) {
    State *s = p;
    s->n = 0;
    if (n == 0) return;
    unsigned at = 1;
    int want = (unsigned char)b[0];
    while (s->n < want && s->n < MAX_PENDING && at + 2 <= n) {
        s->strip[s->n] = b[at++];
        unsigned len = (unsigned char)b[at++];
        if (len > MAX_DELIM || at + len > n) break;
        memcpy(s->delim[s->n], b + at, len);
        s->delim[s->n][len] = 0;
        at += len;
        s->n++;
    }
}

static bool is_ident(int32_t c, bool first) {
    return c == '_' || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (!first && c >= '0' && c <= '9');
}
static bool is_space(int32_t c) { return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == 0; }

// After `<<`: -? and the end word ('END', "END" or END), kept as pending.
static bool heredoc_start(State *s, TSLexer *l) {
    bool strip = false;
    if (l->lookahead == '-') { strip = true; next(l); }
    char d[MAX_DELIM + 1];
    int n = 0;
    if (l->lookahead == '\'' || l->lookahead == '"') {
        int32_t q = l->lookahead;
        next(l);
        while (!l->eof(l) && l->lookahead != q && l->lookahead != '\n') {
            if (n < MAX_DELIM) d[n++] = (char)l->lookahead;
            next(l);
        }
        if (l->lookahead != q || n == 0) return false;
        next(l);
    } else if (is_ident(l->lookahead, true)) {
        // (The word up to a space or one of ;|&<>() and quotes, as bash and
        // tessh's reader have it: End-Of-Session, EOF.txt.)
        while (!l->eof(l) && !is_space(l->lookahead) && !strchr(";|&<>()'\"`", (int)l->lookahead)) {
            if (n < MAX_DELIM) d[n++] = (char)l->lookahead;
            next(l);
        }
    } else {
        return false;
    }
    d[n] = 0;
    if (s->n < MAX_PENDING) {
        s->strip[s->n] = strip;
        memcpy(s->delim[s->n], d, n + 1);
        s->n++;
    }
    return true;
}

// At a line's end with heredocs waiting: their bodies, through each end line.
static bool heredoc_body(State *s, TSLexer *l) {
    if (l->lookahead == '\r') next(l);
    if (l->lookahead != '\n') return false;
    next(l);
    for (int i = 0; i < s->n; i++) {
        for (;;) {
            if (l->eof(l)) { l->mark_end(l); s->n = 0; return true; }
            if (s->strip[i]) while (l->lookahead == '\t') next(l);
            // (Is this line the end word, and nothing else?)
            const char *d = s->delim[i];
            int k = 0;
            while (d[k] && l->lookahead == (unsigned char)d[k]) { next(l); k++; }
            if (!d[k] && (l->lookahead == '\n' || l->lookahead == '\r' || l->eof(l))) {
                l->mark_end(l);
                if (i + 1 < s->n) { if (l->lookahead == '\r') next(l); next(l); }
                break;
            }
            while (!l->eof(l) && l->lookahead != '\n') next(l);
            if (!l->eof(l)) next(l);
        }
    }
    s->n = 0;
    return true;
}

// A redirection at the start of a word: digits, then the operator, and for
// >&/<& the descriptor or -. Not >=, <<, a spaced < or > (comparisons).
static bool redirection(State *s, TSLexer *l, const bool *valid) {
    bool digits = false;
    while (l->lookahead >= '0' && l->lookahead <= '9') { next(l); digits = true; }
    int32_t c = l->lookahead;
    if (c == '&' && !digits) {
        next(l);
        if (l->lookahead != '>') return false;
        next(l);
        if (l->lookahead == '>') next(l);
        l->mark_end(l);
        l->result_symbol = REDIRECTION;
        return valid[REDIRECTION];
    }
    if (c != '<' && c != '>') return false;
    next(l);
    bool spacedok = digits;
    if (c == '<' && l->lookahead == '<') {
        next(l);
        if (l->lookahead == '<') {
            next(l);
            l->mark_end(l);
            l->result_symbol = REDIRECTION;
            return valid[REDIRECTION];
        }
        if (!valid[HEREDOC_START] || !heredoc_start(s, l)) return false;
        l->mark_end(l);
        l->result_symbol = HEREDOC_START;
        return true;
    }
    if (!digits && l->lookahead == '(') {
        next(l);
        l->mark_end(l);
        l->result_symbol = PROCSUB_OPEN;
        return valid[PROCSUB_OPEN];
    }
    if (l->lookahead == '&') {
        next(l);
        if (l->lookahead == '-') {
            next(l);
        } else if (l->lookahead >= '0' && l->lookahead <= '9') {
            while (l->lookahead >= '0' && l->lookahead <= '9') next(l);
        } else if (c == '<') {
            return false;
        }
        // (>&file: both to it; the file is the next word.)
        l->mark_end(l);
        l->result_symbol = REDIRECTION;
        return valid[REDIRECTION];
    }
    if (c == '>' && (l->lookahead == '>' || l->lookahead == '|')) { next(l); spacedok = true; }
    else if (c == '<' && l->lookahead == '>') { next(l); spacedok = true; }
    if (l->lookahead == '=' || l->lookahead == '<' || l->lookahead == '>') return false;
    // (A lone spaced > or <: maybe a comparison; the operator it is.)
    if (is_space(l->lookahead) && !spacedok) return false;
    l->mark_end(l);
    l->result_symbol = REDIRECTION;
    return valid[REDIRECTION];
}

bool tree_sitter_tessh_external_scanner_scan(void *p, TSLexer *l, const bool *valid) {
    State *s = p;
    // (Error recovery asks for all of them, the sentinel too: leave it to the grammar.)
    if (valid[ERROR_SENTINEL]) return false;
    while (l->lookahead == ' ' || l->lookahead == '\t') l->advance(l, true);
    if (s->n > 0 && valid[HEREDOC_BODY] && (l->lookahead == '\n' || l->lookahead == '\r')) {
        if (!heredoc_body(s, l)) return false;
        l->result_symbol = HEREDOC_BODY;
        return true;
    }
    if (valid[DQ_STRING] && l->lookahead == '"') {
        next(l);
        if (!dq_body(l, 0)) return false;
        l->result_symbol = DQ_STRING;
        return true;
    }
    if (valid[REDIRECTION] || valid[PROCSUB_OPEN] || valid[HEREDOC_START]) {
        int32_t c = l->lookahead;
        if ((c >= '0' && c <= '9') || c == '<' || c == '>' || c == '&') return redirection(s, l, valid);
    }
    return false;
}
