#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
#include <tree_sitter/api.h>

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!library) { fprintf(stderr, "%s\n", dlerror()); return 2; }
    const TSLanguage *(*language)(void) = dlsym(library, argv[2]);
    TSParser *parser = ts_parser_new();
    if (!language || !ts_parser_set_language(parser, language())) return 2;
    TSTree *tree = ts_parser_parse_string(parser, NULL, argv[3], strlen(argv[3]));
    if (!tree || ts_node_has_error(ts_tree_root_node(tree))) return 1;
    ts_tree_delete(tree);
    ts_parser_delete(parser);
    dlclose(library);
    return 0;
}
