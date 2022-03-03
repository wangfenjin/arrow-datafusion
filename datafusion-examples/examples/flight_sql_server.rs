// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use std::sync::Arc;

use arrow_flight::{FlightEndpoint, SchemaAsIpc};
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::ListingOptions;
use datafusion::datasource::object_store::local::LocalFileSystem;
use protobuf::well_known_types::Any;
use protobuf::Message;
use tonic::transport::Server;
use tonic::{Response, Status};

use datafusion::prelude::*;

use arrow_flight::{
    flight_service_server::FlightService,
    flight_service_server::FlightServiceServer,
    sql::{
        server::FlightSqlService, ActionClosePreparedStatementRequest,
        ActionCreatePreparedStatementRequest, CommandGetCatalogs,
        CommandGetCrossReference, CommandGetDbSchemas, CommandGetExportedKeys,
        CommandGetImportedKeys, CommandGetPrimaryKeys, CommandGetSqlInfo,
        CommandGetTableTypes, CommandGetTables, CommandPreparedStatementQuery,
        CommandPreparedStatementUpdate, CommandStatementQuery, CommandStatementUpdate,
        TicketStatementQuery,
    },
    FlightData, FlightDescriptor, FlightInfo, Ticket,
};

#[derive(Clone)]
pub struct FlightSqlServiceImpl {
    parquet_file: String,
}

impl FlightSqlServiceImpl {
    fn new() -> Self {
        let testdata = datafusion::arrow::util::test_util::parquet_test_data();
        FlightSqlServiceImpl {
            parquet_file: format!("{}/alltypes_plain.parquet", testdata),
        }
    }
}

#[tonic::async_trait]
impl FlightSqlService for FlightSqlServiceImpl {
    type FlightService = FlightSqlServiceImpl;
    // get_flight_info
    async fn get_flight_info_statement(
        &self,
        query: CommandStatementQuery,
        request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        let listing_options = ListingOptions::new(Arc::new(ParquetFormat::default()));

        let schema = listing_options
            .infer_schema(Arc::new(LocalFileSystem {}), &self.parquet_file)
            .await
            .unwrap();

        let mut ticket = TicketStatementQuery::new();
        ticket.set_statement_handle(query.query.into_bytes());
        let bytes = Any::pack(&ticket)
            .unwrap()
            .write_to_bytes()
            .map_err(|err| Status::internal(err.to_string()))?;
        let endpoint = FlightEndpoint {
            ticket: Some(Ticket { ticket: bytes }),
            location: vec![],
        };
        let info = FlightInfo {
            schema: schema.to_string().into_bytes(),
            endpoint: vec![endpoint],
            total_bytes: -1,
            total_records: -1,
            flight_descriptor: Some(request),
        };
        Ok(Response::new(info))
    }
    async fn get_flight_info_prepared_statement(
        &self,
        _query: CommandPreparedStatementQuery,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_catalogs(
        &self,
        _query: CommandGetCatalogs,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_schemas(
        &self,
        _query: CommandGetDbSchemas,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_tables(
        &self,
        _query: CommandGetTables,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_table_types(
        &self,
        _query: CommandGetTableTypes,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_sql_info(
        &self,
        _query: CommandGetSqlInfo,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_primary_keys(
        &self,
        _query: CommandGetPrimaryKeys,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn get_flight_info_cross_reference(
        &self,
        _query: CommandGetCrossReference,
        _request: FlightDescriptor,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    // do_get
    async fn do_get_statement(
        &self,
        ticket: TicketStatementQuery,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        match std::str::from_utf8(ticket.get_statement_handle()) {
            Ok(sql) => {
                println!("do_get_statement: {}", sql);

                // create local execution context
                let mut ctx = ExecutionContext::new();

                // register parquet file with the execution context
                ctx.register_parquet("alltypes_plain", &self.parquet_file)
                    .await
                    .map_err(to_tonic_err)?;
                // create the DataFrame
                let df = ctx.sql(sql).await.map_err(to_tonic_err)?;
                // execute the query
                let results = df.collect().await.map_err(to_tonic_err)?;
                if results.is_empty() {
                    return Err(Status::internal("There were no results from ticket"));
                }

                // add an initial FlightData message that sends schema
                let options = datafusion::arrow::ipc::writer::IpcWriteOptions::default();
                let schema_flight_data =
                    SchemaAsIpc::new(&df.schema().clone().into(), &options).into();

                let mut flights: Vec<Result<FlightData, Status>> =
                    vec![Ok(schema_flight_data)];

                let mut batches: Vec<Result<FlightData, Status>> = results
                    .iter()
                    .flat_map(|batch| {
                        let (flight_dictionaries, flight_batch) =
                            arrow_flight::utils::flight_data_from_arrow_batch(
                                batch, &options,
                            );
                        flight_dictionaries
                            .into_iter()
                            .chain(std::iter::once(flight_batch))
                            .map(Ok)
                    })
                    .collect();

                // append batch vector to schema vector, so that the first message sent is the schema
                flights.append(&mut batches);

                let output = futures::stream::iter(flights);

                Ok(Response::new(Box::pin(output) as Self::DoGetStream))
            }
            Err(e) => Err(Status::invalid_argument(format!("Invalid ticket: {:?}", e))),
        }
    }

    async fn do_get_prepared_statement(
        &self,
        _query: CommandPreparedStatementQuery,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_catalogs(
        &self,
        _query: CommandGetCatalogs,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_schemas(
        &self,
        _query: CommandGetDbSchemas,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_tables(
        &self,
        _query: CommandGetTables,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_table_types(
        &self,
        _query: CommandGetTableTypes,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_sql_info(
        &self,
        _query: CommandGetSqlInfo,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_primary_keys(
        &self,
        _query: CommandGetPrimaryKeys,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_exported_keys(
        &self,
        _query: CommandGetExportedKeys,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_imported_keys(
        &self,
        _query: CommandGetImportedKeys,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_get_cross_reference(
        &self,
        _query: CommandGetCrossReference,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    // do_put
    async fn do_put_statement_update(
        &self,
        _ticket: CommandStatementUpdate,
    ) -> Result<Response<<Self as FlightService>::DoPutStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_put_prepared_statement_query(
        &self,
        _query: CommandPreparedStatementQuery,
    ) -> Result<Response<<Self as FlightService>::DoPutStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_put_prepared_statement_update(
        &self,
        _query: CommandPreparedStatementUpdate,
    ) -> Result<Response<<Self as FlightService>::DoPutStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    // do_action
    async fn do_action_create_prepared_statement(
        &self,
        _query: ActionCreatePreparedStatementRequest,
    ) -> Result<Response<<Self as FlightService>::DoActionStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
    async fn do_action_close_prepared_statement(
        &self,
        _query: ActionClosePreparedStatementRequest,
    ) -> Result<Response<<Self as FlightService>::DoActionStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
}

fn to_tonic_err(e: datafusion::error::DataFusionError) -> Status {
    Status::internal(format!("{:?}", e))
}

/// This example shows how to wrap DataFusion with `FlightService` to support looking up schema information for
/// Parquet files and executing SQL queries against them on a remote server.
/// This example is run along-side the example `flight_client`.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;
    let service = FlightSqlServiceImpl::new();

    let svc = FlightServiceServer::new(service);

    println!("Listening on {:?}", addr);

    Server::builder().add_service(svc).serve(addr).await?;

    Ok(())
}
