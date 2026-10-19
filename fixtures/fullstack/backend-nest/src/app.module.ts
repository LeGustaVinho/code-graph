import { Module } from "@nestjs/common";
import { InvoicesController } from "./invoices/invoices.controller";
import { InvoicesService } from "./invoices/invoices.service";
import { StripeGateway } from "./payments/payment";

@Module({
  controllers: [InvoicesController],
  providers: [InvoicesService, { provide: "PAYMENT_GATEWAY", useClass: StripeGateway }],
})
export class AppModule {}
