PREFIX ex: <https://query.example.org/def/>

INSERT DATA {
  <https://query.example.org/id/carol> a ex:Person ;
    ex:name "Carol" .
} ;
DELETE { ?p ex:name ?old }
INSERT { ?p ex:name "Renamed" }
WHERE { ?p ex:name ?old }
