Golden fixtures vendored from holistics/dbml (packages/dbml-core,
__tests__/examples), Apache-2.0 — https://github.com/holistics/dbml.
Exporter: <dialect>_exporter/{input/*.in.dbml, output/*.out.sql}.
Importer: <dialect>_importer/{input/*.in.sql, output/*.out.dbml}.
They pin the exact conventions our SQL exporter and importer must
reproduce; see src/dbml/sql.rs and src/dbml/sqldump.rs.
