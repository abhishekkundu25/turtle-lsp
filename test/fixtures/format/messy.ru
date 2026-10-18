prefix ex: <https://query.example.org/def/>
insert data{<https://query.example.org/id/carol> a ex:Person;ex:name "Carol".
graph <https://query.example.org/graph/people>{<https://query.example.org/id/carol> ex:age 42}};
# rename everyone
delete{?p ex:name ?old}insert{?p ex:name "Renamed"}where{?p ex:name ?old};
with <https://query.example.org/graph/people> delete {?p ex:age ?a} where {?p ex:age ?a};
load silent <http://e.org/data> into graph <http://e.org/g> ;   clear all;drop silent graph <http://e.org/g>
