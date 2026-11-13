# Additional queries

For public APIs, use `get_file_skeleton(file_path,include_private?)` and `get_type_contract(type_id)`. Use `get_type_hierarchy(symbol_id)` for inheritance and `get_entrypoints` for indexed entrypoints.

For dependencies and behavior, use `resolve_dependency_injection(interface_or_token)`, `get_state_mutations(symbol_id,include_callees?)`, `trace_error_flow(exception_name_or_message)`, or `get_config_matrix(filter?,path?)`.
