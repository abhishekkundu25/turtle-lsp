{
  "@context": {
    "ex": "http://example.org/",
    "foaf": "http://xmlns.com/foaf/0.1/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "schema": "https://schema.org/",
    "xsd": "http://www.w3.org/2001/XMLSchema#",
    "name": "foaf:name",
    "knows": {
      "@id": "foaf:knows",
      "@type": "@id"
    },
    "born": {
      "@id": "schema:birthDate",
      "@type": "xsd:date"
    },
    "comment": {
      "@id": "rdfs:comment",
      "@language": "en"
    },
    "address": "schema:address",
    "skills": {
      "@id": "ex:skills",
      "@container": "@list"
    }
  },
  "@graph": [
@@
    {
      "@id": "ex:p%TAG%%I%",
      "@type": "foaf:Person",
      "name": "Person %I%",
      "born": "19%BORN%-01-01",
      "comment": "A longer description of person %I%",
      "knows": [
        "ex:p%TAG%%K1%",
        "ex:p%TAG%%K2%"
      ],
      "address": {
        "@type": "schema:PostalAddress",
        "schema:addressLocality": "Town %TOWN%"
      },
      "skills": [
        "rdf",
        "sparql",
        "skill-%SKILL%"
      ]
    }
@@
  ]
}
