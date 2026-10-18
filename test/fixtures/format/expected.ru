PREFIX ex: <https://query.example.org/def/>

INSERT DATA {
    <https://query.example.org/id/carol> a ex:Person ;
        ex:name "Carol" .
    GRAPH <https://query.example.org/graph/people> {
        <https://query.example.org/id/carol> ex:age 42
    }
} ;

# rename everyone
DELETE {
    ?p ex:name ?old
}
INSERT {
    ?p ex:name "Renamed"
}
WHERE {
    ?p ex:name ?old
} ;

WITH <https://query.example.org/graph/people>
DELETE {
    ?p ex:age ?a
}
WHERE {
    ?p ex:age ?a
} ;

LOAD SILENT <http://e.org/data> INTO GRAPH <http://e.org/g> ;

CLEAR ALL ;

DROP SILENT GRAPH <http://e.org/g>
